//! Session endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{ApiError, ApiResult, AppState};
use crate::hooks::HookEvent;
use crate::vault::sessions;

/// `GET /api/v1/sessions`
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let items = sessions::list(&st.pool).await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct CreateBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// `POST /api/v1/sessions`
pub async fn create(
    State(st): State<AppState>,
    Json(body): Json<CreateBody>,
) -> ApiResult<(StatusCode, Json<sessions::Session>)> {
    let id = body
        .id
        .unwrap_or_else(|| format!("s-{}", uuid::Uuid::new_v4().simple()));
    let session = sessions::create(&st.pool, &id, body.title.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

#[derive(Deserialize)]
pub struct RenameBody {
    pub title: String,
}

/// `PATCH /api/v1/sessions/{id}`
pub async fn rename(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> ApiResult<StatusCode> {
    if sessions::rename(&st.pool, &id, &body.title).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("session '{id}' not found")))
    }
}

/// `DELETE /api/v1/sessions/{id}`
pub async fn remove(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if sessions::delete(&st.pool, &id).await? {
        // ── SessionEnd hook (M2.5) — advisory; deletion has already happened.
        if st.hooks.has_event(HookEvent::SessionEnd) {
            let payload = serde_json::json!({
                "session_id": id,
                "hook_event_name": "SessionEnd",
                "session_end_reason": "manual",
            });
            let _ = st.hooks.trigger(HookEvent::SessionEnd, "manual", payload).await;
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("session '{id}' not found")))
    }
}

/// `POST /api/v1/sessions/{session_id}/turns/{turn_id}/abort` (M3.2).
///
/// Wakes the agent loop's abort `Notify` so it bails with
/// `stop_reason = "user_aborted"`. Returns 204 on a successful wake (the
/// notify was registered → the loop is running and will see it next time
/// it polls `select!`), 404 if no notify is registered (turn already
/// exited, or never existed). The session-id path segment is checked for
/// existence to give the frontend a clearer error for a typo'd session.
///
/// This endpoint does *not* wait for the loop to actually finish — the
/// caller listens on the SSE stream for `assistant_done` with
/// `stop_reason = "user_aborted"` to confirm.
pub async fn abort_turn(
    State(st): State<AppState>,
    Path((session_id, turn_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    // A missing session is a clearer 404 than the generic "no abort signal"
    // (helps the frontend distinguish "user pasted a bad session id" from
    // "turn finished a microsecond before you clicked").
    if !sessions::exists(&st.pool, &session_id).await? {
        return Err(ApiError::not_found(format!(
            "session '{session_id}' not found"
        )));
    }
    let Some(notify) = st.lookup_abort_signal(&turn_id) else {
        return Err(ApiError::not_found(format!(
            "no running turn '{turn_id}' in session '{session_id}'"
        )));
    };
    notify.notify_one();
    tracing::info!(session_id, turn_id, "user abort requested");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    //! Backend tests for the M3.2 abort endpoint.
    //!
    //! Real end-to-end (POST → agent loop wake → `stop_reason="user_aborted"`
    //! in turn_metrics) needs a mocked codex stream which the rest of the
    //! suite does not have. We pin the algorithm shape instead: register a
    //! notify, fire from the endpoint, observe that the same `Arc<Notify>`
    //! the loop would hold is awoken. Plus the 404 paths.
    use super::*;
    use crate::bus::EventBus;
    use crate::config::Config;
    use crate::llm::codex::CodexClient;
    use crate::vault::Vault;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    async fn fixture_state() -> AppState {
        let path = std::env::temp_dir().join(format!(
            "leek-api-sessions-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let vault = Vault::open(&path).await.unwrap();
        let codex = CodexClient::new(vault.pool.clone(), crate::vault::LOCAL_USER).unwrap();
        let corpus = Arc::new(crate::corpus::Corpus::empty());
        let corpus_graph = Arc::new(corpus.build_graph());
        AppState {
            pool: vault.pool,
            bus: EventBus::new(),
            codex,
            http: reqwest::Client::new(),
            config: Arc::new(RwLock::new(Config::default())),
            web_search: false,
            corpus,
            corpus_graph,
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            hooks: Arc::new(crate::hooks::HookEngine::default()),
            agents: Arc::new(crate::agents::AgentRegistry::default()),
            vendors: Arc::new(crate::vendors::VendorRegistry::for_test()),
            abort_signals: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn register_and_remove_round_trip() {
        let st = fixture_state().await;
        assert!(st.lookup_abort_signal("t-1").is_none());
        let _notify = st.register_abort_signal("t-1");
        assert!(st.lookup_abort_signal("t-1").is_some());
        st.remove_abort_signal("t-1");
        assert!(st.lookup_abort_signal("t-1").is_none());
    }

    /// Registering and firing wakes the same `Arc<Notify>` the loop is
    /// holding. We hold an `Arc<Notify>` ourselves and call the endpoint;
    /// `notified()` should resolve. Without the wake, the wait stays
    /// pending — so we wrap the `notified()` in a 1s timeout that would
    /// fire otherwise.
    #[tokio::test]
    async fn endpoint_wakes_the_loops_arc() {
        let st = fixture_state().await;
        // The agent loop creates its session via `sessions::ensure`;
        // mirror that here so the endpoint's exists() check passes.
        sessions::ensure(&st.pool, "s-test").await.unwrap();
        let notify = st.register_abort_signal("t-test");

        // Fire the endpoint.
        let status = abort_turn(
            State(st.clone()),
            Path(("s-test".to_string(), "t-test".to_string())),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::NO_CONTENT);

        // The loop's `notify.notified()` should resolve almost immediately.
        let woke = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            notify.notified(),
        )
        .await;
        assert!(woke.is_ok(), "notify.notified() did not resolve after endpoint fire");
    }

    #[tokio::test]
    async fn endpoint_404s_for_unknown_session() {
        let st = fixture_state().await;
        let err = abort_turn(
            State(st),
            Path(("does-not-exist".to_string(), "t-1".to_string())),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert!(err.message.contains("does-not-exist"));
    }

    #[tokio::test]
    async fn endpoint_404s_for_unknown_turn() {
        let st = fixture_state().await;
        sessions::ensure(&st.pool, "s-test").await.unwrap();
        let err = abort_turn(
            State(st),
            Path(("s-test".to_string(), "t-does-not-exist".to_string())),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert!(err.message.contains("t-does-not-exist"));
    }

    /// The drop-guard pattern in `agent::mod::drive` — even if the agent
    /// future is cancelled (loop drops mid-iteration), the slot must be
    /// cleared. We simulate by registering, calling `remove_abort_signal`
    /// from a scope drop, and verifying the slot is gone.
    #[tokio::test]
    async fn drop_pattern_clears_slot() {
        let st = fixture_state().await;
        {
            let _notify = st.register_abort_signal("t-drop");
            assert!(st.lookup_abort_signal("t-drop").is_some());
            // Mirror what `AbortGuard` does in its Drop impl.
            st.remove_abort_signal("t-drop");
        }
        assert!(st.lookup_abort_signal("t-drop").is_none());
    }
}
