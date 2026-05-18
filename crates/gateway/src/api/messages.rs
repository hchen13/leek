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
        "message_created",
        serde_json::json!({
            "seq": user.seq,
            "role": "user",
            "content": user.content,
            "created_at": user.created_at,
        }),
    )
    .await;

    let turn_id = format!("turn-{}", uuid::Uuid::new_v4().simple());
    tokio::spawn(crate::agent::run_turn(
        st.clone(),
        session_id.clone(),
        turn_id.clone(),
    ));

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "turn_id": turn_id, "user_seq": user.seq })),
    ))
}
