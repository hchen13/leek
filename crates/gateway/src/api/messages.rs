//! Message endpoints. `post` runs the M0 echo worker.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{ApiResult, AppState};
use crate::vault::{events, messages, sessions};

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
/// Persists the user message, then runs the M0 echo worker: a fixed
/// `Echo: <text>` assistant reply. Every step also lands as a row in `events`
/// and is fanned out over SSE.
pub async fn post(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PostBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    sessions::ensure(&st.pool, &session_id).await?;

    // User message.
    let user = messages::insert(&st.pool, &session_id, "user", &body.content).await?;
    emit(
        &st,
        &session_id,
        "message_created",
        serde_json::json!({
            "seq": user.seq,
            "role": "user",
            "content": user.content,
            "created_at": user.created_at,
        }),
    )
    .await?;

    // Echo worker — M0's stand-in for the agent loop.
    let reply = format!("Echo: {}", body.content);
    emit(
        &st,
        &session_id,
        "assistant_delta",
        serde_json::json!({ "text": reply }),
    )
    .await?;
    let assistant = messages::insert(&st.pool, &session_id, "assistant", &reply).await?;
    emit(
        &st,
        &session_id,
        "message_created",
        serde_json::json!({
            "seq": assistant.seq,
            "role": "assistant",
            "content": assistant.content,
            "created_at": assistant.created_at,
        }),
    )
    .await?;
    emit(
        &st,
        &session_id,
        "assistant_done",
        serde_json::json!({ "message_seq": assistant.seq }),
    )
    .await?;

    sessions::touch(&st.pool, &session_id).await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "user_seq": user.seq,
            "assistant_seq": assistant.seq,
        })),
    ))
}

/// Persist an event, then fan it out to live SSE subscribers.
async fn emit(
    st: &AppState,
    session_id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> ApiResult<()> {
    let event = events::insert(&st.pool, session_id, kind, &payload).await?;
    st.bus.publish(session_id, event).await;
    Ok(())
}
