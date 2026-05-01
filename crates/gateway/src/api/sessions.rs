//! Session-level operations.
//!
//! `abort_handler` cancels the in-flight agent reply for the session — the
//! token is signalled, the reply pipeline graceful-exits at the next
//! `select!` point and commits whatever partial text it has accumulated.

use axum::extract::{Path, State};
use axum::http::StatusCode;

use super::AppState;

pub async fn abort_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> StatusCode {
    let map = state.active_replies.lock().await;
    if let Some(token) = map.get(&session_id) {
        token.cancel();
        tracing::info!(session_id, "abort signalled");
    }
    StatusCode::ACCEPTED
}
