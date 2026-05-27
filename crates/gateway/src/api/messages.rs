//! Message endpoints. `post` persists the user message and spawns an
//! agent turn (M0's echo worker is gone — M1 runs the real loop).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{ApiResult, AppState};
use crate::vault::{messages, sessions};

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /api/v1/sessions/{id}/messages`
pub async fn list(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = messages::list(&st.pool, &session_id, q.since, q.limit.unwrap_or(200)).await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct PostBody {
    pub content: String,
}

/// `POST /api/v1/sessions/{id}/messages`
///
/// Persists the user message, then spawns the agent turn in the background
/// and returns `202 Accepted` immediately. A real turn runs for seconds to
/// minutes, so the response carries only the `turn_id`; the client watches
/// the SSE stream (`assistant_delta`, `tool_call`, `tool_result`,
/// `assistant_done`, `turn_metrics_recorded`) for the turn's progress.
pub async fn post(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PostBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    sessions::ensure(&st.pool, &session_id).await?;

    let user = messages::insert(&st.pool, &session_id, "user", &body.content).await?;
    st.emit(
        &session_id,
        crate::agent::events::kind::MESSAGE_CREATED,
        serde_json::json!({
            "seq": user.seq,
            "role": "user",
            "content": user.content,
            "created_at": user.created_at,
        }),
    )
    .await;

    let turn_id = format!("turn-{}", uuid::Uuid::new_v4().simple());
    spawn_turn(st.clone(), session_id.clone(), turn_id.clone());

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "turn_id": turn_id, "user_seq": user.seq })),
    ))
}

/// Spawn the agent turn under a panic guard.
///
/// `run_turn` is fire-and-forget on the tokio runtime — a panic inside
/// (anywhere on the call tree, including vendor-string slicing landing
/// mid-CJK-character — see the `vendors::types::Symbol::parse`
/// regression test) would be swallowed by tokio's default panic
/// behavior and leave the frontend hanging without an `assistant_done`
/// or `error` event. `catch_unwind` turns the panic into a structured
/// `ERROR` event so the SSE stream always closes out the turn.
fn spawn_turn(st: AppState, session_id: String, turn_id: String) {
    use futures::FutureExt;
    tokio::spawn(async move {
        let fut = std::panic::AssertUnwindSafe(crate::agent::run_turn(
            st.clone(),
            session_id.clone(),
            turn_id.clone(),
        ));
        if let Err(panic) = fut.catch_unwind().await {
            let msg = panic_message(panic.as_ref());
            tracing::error!(
                session_id = %session_id,
                turn_id = %turn_id,
                panic = %msg,
                "agent turn task panicked"
            );
            st.emit(
                &session_id,
                crate::agent::events::kind::ERROR,
                serde_json::json!({
                    "turn_id": turn_id,
                    "message": format!("agent turn panicked: {msg}"),
                    "kind": "panic",
                }),
            )
            .await;
        }
    });
}

/// Extract a printable message from a `Box<dyn Any + Send>` payload
/// returned by `catch_unwind`. Std-library panics carry either a
/// `&'static str` or a `String`; anything else falls back to a generic
/// label.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::config::Config;
    use crate::llm::codex::CodexClient;
    use crate::vault::Vault;
    use futures::FutureExt;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};

    async fn fixture_state() -> AppState {
        let path = std::env::temp_dir().join(format!(
            "leek-api-messages-test-{}.db",
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
            codex_sem: Arc::new(tokio::sync::Semaphore::new(crate::api::CODEX_MAX_CONCURRENT)),
        }
    }

    #[test]
    fn panic_message_extracts_str_payload() {
        let r = std::panic::catch_unwind(|| {
            panic!("static str panic");
        });
        let msg = panic_message(r.unwrap_err().as_ref());
        assert!(msg.contains("static str panic"), "got: {msg}");
    }

    #[test]
    fn panic_message_extracts_string_payload() {
        let r = std::panic::catch_unwind(|| {
            let s = String::from("dynamic string panic");
            panic!("{s}");
        });
        let msg = panic_message(r.unwrap_err().as_ref());
        assert!(msg.contains("dynamic string panic"), "got: {msg}");
    }

    /// End-to-end of the panic guard: a spawned future panicking inside
    /// `catch_unwind` must NOT silently abort the task. We use the same
    /// pattern `spawn_turn` uses (AssertUnwindSafe + catch_unwind) and
    /// confirm the panic surfaces as `Err(_)` rather than tearing down
    /// the worker.
    #[tokio::test]
    async fn catch_unwind_traps_panic_in_spawned_future() {
        let h = tokio::spawn(async move {
            let fut = std::panic::AssertUnwindSafe(async {
                // Mimic the real panic site: byte-indexing into a CJK
                // string at a non-boundary. If `Symbol::parse` ever
                // regresses, this is the shape of the panic that would
                // reach `spawn_turn`.
                let s = "化学制药";
                let _ = &s[..2];
            });
            fut.catch_unwind().await
        });
        let result = h.await.expect("join should succeed");
        assert!(
            result.is_err(),
            "expected catch_unwind to capture the byte-boundary panic"
        );
        let msg = panic_message(result.unwrap_err().as_ref());
        assert!(
            msg.contains("char boundary"),
            "expected the panic message to mention the byte/char boundary; got: {msg}"
        );
    }

    /// Concrete check that the `spawn_turn` panic path emits an `ERROR`
    /// event so the frontend SSE doesn't hang silently. We can't easily
    /// run `run_turn` end-to-end (it pulls codex), so this re-implements
    /// the panic guard against a future that panics immediately and
    /// asserts the event is published on the bus.
    #[tokio::test]
    async fn spawn_turn_panic_emits_error_event() {
        let st = fixture_state().await;
        let session_id = "s-panic-test".to_string();
        let turn_id = "turn-panic-test".to_string();
        // `emit` durably inserts the event row, which has a FK to
        // sessions — bootstrap the parent row first.
        crate::vault::sessions::ensure(&st.pool, &session_id)
            .await
            .unwrap();

        // Subscribe BEFORE spawning so we don't race the emit.
        let mut rx = st.bus.subscribe(&session_id).await;

        // Mirror the production `spawn_turn` body but against a future
        // that we know will panic.
        let st_c = st.clone();
        let sid = session_id.clone();
        let tid = turn_id.clone();
        let handle = tokio::spawn(async move {
            let fut = std::panic::AssertUnwindSafe(async {
                let s = "化学制药";
                let _ = &s[..2]; // panic at byte 2 inside `化`
            });
            if let Err(panic) = fut.catch_unwind().await {
                let msg = panic_message(panic.as_ref());
                st_c.emit(
                    &sid,
                    crate::agent::events::kind::ERROR,
                    serde_json::json!({
                        "turn_id": tid,
                        "message": format!("agent turn panicked: {msg}"),
                        "kind": "panic",
                    }),
                )
                .await;
            }
        });
        handle.await.expect("spawned task must not propagate panic");

        // The subscribe() above is a broadcast channel; the emit lands
        // in the receiver. Pull the next event and verify shape.
        let env = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for error event")
            .expect("event bus closed");
        assert_eq!(env.kind, crate::agent::events::kind::ERROR);
        assert_eq!(env.payload["kind"], "panic");
        assert_eq!(env.payload["turn_id"], turn_id);
        let msg = env.payload["message"].as_str().unwrap_or("");
        assert!(
            msg.contains("char boundary"),
            "expected panic message in event; got: {msg}"
        );
    }
}
