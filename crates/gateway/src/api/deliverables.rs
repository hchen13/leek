use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{AppError, AppState};
use crate::vault::decisions as vault_decisions;

#[derive(Deserialize)]
pub struct ListDeliverablesQuery {
    pub status: Option<String>,
}

pub async fn list_handler(
    State(state): State<AppState>,
    Query(q): Query<ListDeliverablesQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let status = q.status.as_deref().unwrap_or("pending_confirm");
    let rows = if status == "pending_confirm" {
        vault_decisions::list_pending(&state.pool, &state.user_id).await?
    } else {
        return Ok(Json(serde_json::json!({ "deliverables": [] })));
    };
    Ok(Json(serde_json::json!({ "deliverables": rows })))
}

pub async fn confirm_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let decision_id =
        vault_decisions::confirm_deliverable(&state.pool, &state.user_id, &id).await?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "decision_id": decision_id })),
    ))
}

#[derive(Deserialize)]
pub struct RejectBody {
    pub reason: Option<String>,
}

pub async fn reject_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RejectBody>,
) -> Result<StatusCode, AppError> {
    vault_decisions::reject_deliverable(&state.pool, &state.user_id, &id, body.reason.as_deref())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn decisions_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows = vault_decisions::list_decisions(&state.pool, &state.user_id, 100).await?;
    Ok(Json(serde_json::json!({ "decisions": rows })))
}
