//! `GET / PATCH /api/v1/settings` — the user-facing settings surface (M2.6).
//!
//! - `GET` returns both layers: the raw config-file values (so the UI can
//!   show "user has set this to X") *and* the `effective` values that
//!   `GuardConfig::resolve` actually applies (so the UI can show "the env
//!   var is overriding your X"). The frontend uses the diff between the
//!   two to render the "⚠ overridden" badge in §B.2.
//! - `PATCH` accepts a partial `Config` document, merges it onto the live
//!   in-memory `Config`, writes the merged result to `~/.leek/config.json`,
//!   and returns the same shape as `GET` so the UI can refresh without a
//!   second round-trip. The next agent turn re-runs `GuardConfig::resolve`
//!   and picks the new values up — no restart needed.
//!
//! Validation is "shape only": `Config::validate` checks numeric ranges
//! (no negative cost cap, threshold ≥ 2, etc.) and the handler returns 400
//! with a per-field error list. Anything outside numeric sanity (does the
//! user *want* a 0.01¢ cap?) is the user's call.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde_json::json;

use super::{ApiError, ApiResult, AppState};
use crate::agent::GuardConfig;
use crate::config::Config;

/// `GET /api/v1/settings` — returns `{ config, effective }`.
///
/// `config` is what the user has saved to `~/.leek/config.json` (each field
/// is `null` if unset). `effective` is the values `GuardConfig::resolve`
/// produces after layering env vars on top, plus a `*_overridden` boolean
/// per field so the UI can render the override badge without re-implementing
/// the resolver.
pub async fn read(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let cfg = st.config_snapshot();
    Ok(Json(response_body(&cfg)))
}

/// `PATCH /api/v1/settings` — merge a partial document and persist.
///
/// `Json<serde_json::Value>` (not `Json<Config>`) so we control the 400
/// message ourselves: a stray `{"cost_cap_usd": "free"}` would otherwise
/// produce an axum-default JSON error, which is harder to act on in the UI.
pub async fn patch(
    State(st): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    // The body must be a JSON object. Anything else (top-level array,
    // number, etc.) is a 400 we can phrase clearly.
    if !body.is_object() {
        return Err(ApiError::bad_request(
            "request body must be a JSON object of settings fields",
        ));
    }
    // Parse via the typed shape so we get serde's per-field type errors.
    let patch: Config = serde_json::from_value(body).map_err(|e| {
        ApiError::bad_request(format!("invalid settings document: {e}"))
    })?;

    // Validate the patch on its own first — a malformed value should be
    // rejected before we mutate any state. (The merged result is also
    // validated below, since a merge could theoretically interact across
    // fields; today there's no such interaction, but the second check is
    // cheap and future-proofs the handler.)
    if let Err(errors) = patch.validate() {
        return Err(bad_request_with_fields(&errors));
    }

    // Merge under the lock, then drop it before doing I/O. We never await
    // while holding the lock (it's `std::sync::RwLock`, not tokio).
    let merged = {
        let mut guard = st.config.write().map_err(|_| {
            ApiError::bad_request("settings lock is poisoned; restart the server")
        })?;
        guard.merge(patch);
        guard.clone()
    };

    // Validate the merged document — same check, defensive belt + braces.
    if let Err(errors) = merged.validate() {
        return Err(bad_request_with_fields(&errors));
    }

    // Persist atomically. A failed save reverts the in-memory state so a
    // disk error doesn't leave the live config drifting from the file.
    if let Err(e) = merged.save() {
        // Re-load from disk under the write lock — if the file is also
        // unreadable, fall back to whatever load() returns (which itself
        // has graceful fallback). Net effect: a save error doesn't leave
        // the gateway in an inconsistent state.
        if let Ok(mut guard) = st.config.write() {
            *guard = Config::load();
        }
        tracing::error!(error = ?e, "failed to persist settings; reverted in-memory state");
        return Err(ApiError::from(e));
    }

    Ok((StatusCode::OK, Json(response_body(&merged))))
}

/// Shared body shape for `GET` and the `PATCH` response. See `read` for
/// what each field means.
fn response_body(cfg: &Config) -> serde_json::Value {
    let effective = GuardConfig::resolve(cfg);
    json!({
        "config": cfg,
        "effective": effective_body(cfg, &effective),
        "config_path": Config::path().display().to_string(),
    })
}

/// Render the `effective` view: actual values applied + an `overridden`
/// flag per field. The flag is `true` iff a corresponding env var is set
/// (regardless of value or validity) — that's the surface the UI cares
/// about: "is my config being shadowed?". `_cfg` is unused today but kept
/// in the signature so a future "config layer set this field" badge has a
/// place to read from without changing call sites.
fn effective_body(_cfg: &Config, eff: &GuardConfig) -> serde_json::Value {
    let env_set = |k: &str| std::env::var(k).is_ok();
    json!({
        "idle_timeout_secs": {
            "value": eff.idle_timeout.map(|d| d.as_secs()),
            "overridden_by_env": env_set("LEEK_IDLE_TIMEOUT_SECS"),
        },
        "wall_clock_secs": {
            "value": eff.wall_clock.map(|d| d.as_secs()),
            "overridden_by_env": env_set("LEEK_WALL_CLOCK_SECS"),
        },
        "max_iterations": {
            "value": eff.max_iterations,
            "overridden_by_env": env_set("LEEK_MAX_ITERATIONS"),
        },
        "cost_cap_usd": {
            "value": eff.cost_cap_usd,
            "overridden_by_env": env_set("LEEK_COST_CAP_USD"),
        },
        "doom_loop_threshold": {
            "value": eff.doom_loop_threshold,
            "overridden_by_env": env_set("LEEK_DOOM_LOOP_THRESHOLD"),
        },
        "auto_compact_threshold": {
            "value": eff.auto_compact_threshold,
            "overridden_by_env": env_set("LEEK_AUTO_COMPACT_THRESHOLD"),
        },
        "context_window": {
            "value": eff.context_window,
            "overridden_by_env": env_set("LEEK_CONTEXT_WINDOW"),
        },
        // The config file says nothing about web_search (a capability flag,
        // not a guard) — but exposing the effective value here is helpful
        // for the UI, even when there is no editable field for it.
        "web_search": {
            // Effective value is the AppState's startup-resolved boolean
            // (env-only today); echo it via `cfg` would be wrong since
            // `Config` doesn't store it. The UI reads this only.
            "value": std::env::var("LEEK_WEB_SEARCH").is_ok()
                && matches!(
                    std::env::var("LEEK_WEB_SEARCH").ok().as_deref(),
                    Some("1" | "true" | "on" | "yes")
                ),
            "overridden_by_env": std::env::var("LEEK_WEB_SEARCH").is_ok(),
        },
    })
}

/// 400 with a structured `errors[]` body. `ApiError`'s default `IntoResponse`
/// wraps a string in `{"error": ...}`; for per-field errors we want the UI
/// to be able to render each one inline, so we serialize the list into a
/// JSON-shaped message that the frontend parses.
fn bad_request_with_fields(errors: &[crate::config::ConfigFieldError]) -> ApiError {
    // Embed the JSON shape in the message — `ApiError::into_response` wraps
    // it once more in `{"error": ...}`, but the frontend just `JSON.parse`s
    // the message string. Keeping `ApiError` simple is worth the small
    // ceremony at the parse site.
    let payload = json!({
        "kind": "validation_failed",
        "errors": errors.iter().map(|e| json!({
            "field": e.field,
            "message": e.message,
        })).collect::<Vec<_>>(),
    });
    ApiError::bad_request(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::llm::codex::CodexClient;
    use crate::vault::Vault;
    use std::sync::{Arc, RwLock};
    use tokio::sync::Mutex;

    // Tests touch process-global state (the CWD-based config dir and a few
    // env vars); serialize them on this mutex so `cargo test` doesn't race.
    // `tokio::sync::Mutex` (not std) so the guard can cross `.await` without
    // tripping clippy's `await_holding_lock`.
    static TEST_LOCK: Mutex<()> = Mutex::const_new(());

    /// Build an isolated `AppState` whose config file lives in a uuid-named
    /// temp dir; restores `LEEK_CONFIG_DIR` (and friends) on drop.
    struct TestState {
        st: AppState,
        _scratch: ScratchDir,
        _env: EnvSnapshot,
    }
    impl std::ops::Deref for TestState {
        type Target = AppState;
        fn deref(&self) -> &AppState {
            &self.st
        }
    }

    struct ScratchDir {
        path: std::path::PathBuf,
    }
    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Snapshot of every env var the settings tests touch, so changes are
    /// always reverted no matter how the test exits.
    struct EnvSnapshot {
        vars: Vec<(&'static str, Option<String>)>,
    }
    impl EnvSnapshot {
        fn capture(keys: &[&'static str]) -> Self {
            let vars = keys.iter().map(|&k| (k, std::env::var(k).ok())).collect();
            // Clear them up front for a known-good baseline.
            for k in keys {
                std::env::remove_var(k);
            }
            Self { vars }
        }
    }
    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.vars {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    async fn fixture() -> TestState {
        let dir = std::env::temp_dir().join(format!(
            "leek-settings-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let env = EnvSnapshot::capture(&[
            "LEEK_CONFIG_DIR",
            "LEEK_IDLE_TIMEOUT_SECS",
            "LEEK_WALL_CLOCK_SECS",
            "LEEK_MAX_ITERATIONS",
            "LEEK_COST_CAP_USD",
            "LEEK_DOOM_LOOP_THRESHOLD",
            "LEEK_AUTO_COMPACT_THRESHOLD",
            "LEEK_CONTEXT_WINDOW",
            "LEEK_WEB_SEARCH",
        ]);
        std::env::set_var("LEEK_CONFIG_DIR", &dir);

        let vault_path = dir.join("vault.db");
        let vault = Vault::open(&vault_path).await.unwrap();
        let codex = CodexClient::new(vault.pool.clone(), crate::vault::LOCAL_USER).unwrap();
        let st = AppState {
            pool: vault.pool,
            bus: EventBus::new(),
            codex,
            http: reqwest::Client::new(),
            config: Arc::new(RwLock::new(Config::default())),
            web_search: false,
            corpus: Arc::new(crate::corpus::Corpus::empty()),
        };
        TestState { st, _scratch: ScratchDir { path: dir }, _env: env }
    }

    #[tokio::test]
    async fn get_returns_default_when_nothing_set() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        let resp = read(State(st.st.clone())).await.unwrap().0;
        // Default Config: every field is null.
        assert!(resp["config"]["cost_cap_usd"].is_null());
        assert!(resp["config"]["idle_timeout_secs"].is_null());
        // Effective: idle_timeout has a 90s built-in default, cost cap stays off.
        assert_eq!(resp["effective"]["idle_timeout_secs"]["value"], 90);
        assert!(resp["effective"]["cost_cap_usd"]["value"].is_null());
        // No env vars set in this fixture.
        assert_eq!(resp["effective"]["cost_cap_usd"]["overridden_by_env"], false);
        // config_path points into our temp dir.
        let p = resp["config_path"].as_str().unwrap();
        assert!(p.ends_with("config.json"), "{p}");
    }

    #[tokio::test]
    async fn patch_writes_the_file_and_updates_live_state() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        let body = json!({ "cost_cap_usd": 0.5 });
        let (_status, resp) = patch(State(st.st.clone()), Json(body)).await.unwrap();
        assert_eq!(resp.0["config"]["cost_cap_usd"], 0.5);
        assert_eq!(resp.0["effective"]["cost_cap_usd"]["value"], 0.5);

        // The file actually exists with the right body.
        let on_disk = std::fs::read_to_string(Config::path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["cost_cap_usd"], 0.5);

        // The next read sees the same value via the live `Arc<RwLock>>.
        let snap = st.st.config_snapshot();
        assert_eq!(snap.cost_cap_usd, Some(0.5));
    }

    #[tokio::test]
    async fn patch_merges_without_overwriting_other_fields() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        patch(State(st.st.clone()), Json(json!({ "cost_cap_usd": 0.5 })))
            .await
            .unwrap();
        patch(State(st.st.clone()), Json(json!({ "idle_timeout_secs": 60 })))
            .await
            .unwrap();

        let snap = st.st.config_snapshot();
        assert_eq!(snap.cost_cap_usd, Some(0.5));
        assert_eq!(snap.idle_timeout_secs, Some(60));
    }

    #[tokio::test]
    async fn patch_rejects_negative_cost_cap() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        let err = patch(State(st.st.clone()), Json(json!({ "cost_cap_usd": -1.0 })))
            .await
            .unwrap_err();
        // 400 with a parseable per-field payload.
        let parsed: serde_json::Value = serde_json::from_str(&err.message).unwrap();
        assert_eq!(parsed["kind"], "validation_failed");
        let errs = parsed["errors"].as_array().unwrap();
        assert!(errs.iter().any(|e| e["field"] == "cost_cap_usd"));
        // The on-disk file is untouched (no half-applied PATCH).
        assert!(!Config::path().exists());
    }

    #[tokio::test]
    async fn patch_rejects_non_object_body() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        let err = patch(State(st.st.clone()), Json(json!([1, 2, 3])))
            .await
            .unwrap_err();
        assert!(err.message.contains("must be a JSON object"));
    }

    #[tokio::test]
    async fn get_reports_env_override_flag() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        // Set an env var that should shadow the config; read should show
        // the env value AND mark the field as `overridden_by_env`.
        std::env::set_var("LEEK_COST_CAP_USD", "2.5");
        let resp = read(State(st.st.clone())).await.unwrap().0;
        assert_eq!(resp["effective"]["cost_cap_usd"]["value"], 2.5);
        assert_eq!(resp["effective"]["cost_cap_usd"]["overridden_by_env"], true);
    }

    #[tokio::test]
    async fn patch_with_zero_disables_cost_cap() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        // First set a non-zero cap, then clear it back to 0 = "off".
        patch(State(st.st.clone()), Json(json!({ "cost_cap_usd": 1.0 })))
            .await
            .unwrap();
        let (_s, resp) = patch(State(st.st.clone()), Json(json!({ "cost_cap_usd": 0.0 })))
            .await
            .unwrap();
        assert_eq!(resp.0["config"]["cost_cap_usd"], 0.0);
        // Effective guard treats 0 as "no cap" → null in the response.
        assert!(resp.0["effective"]["cost_cap_usd"]["value"].is_null());
    }

    #[tokio::test]
    async fn patch_rejects_unknown_field() {
        let _l = TEST_LOCK.lock().await;
        let st = fixture().await;
        let err = patch(
            State(st.st.clone()),
            Json(json!({ "max_cost_usd_per_turn": 1.0 })),
        )
        .await
        .unwrap_err();
        // serde's "unknown field" message is fine for a 400 — the UI will
        // surface it verbatim. We just check the right error path fired.
        assert!(err.message.contains("invalid settings document"));
    }
}
