use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::vault::charters;

use super::{AppError, AppState};

pub async fn get_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = charters::get_active(&state.pool, &state.user_id).await?;
    let charter = row.map(|r| {
        serde_json::json!({
            "id": r.id,
            "text": r.text,
            "version": r.version,
            "updated_at": r.updated_at,
        })
    });
    Ok(Json(serde_json::json!({ "charter": charter })))
}

#[derive(Deserialize)]
pub struct PutCharterBody {
    pub text: String,
}

pub async fn put_handler(
    State(state): State<AppState>,
    Json(body): Json<PutCharterBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = charters::upsert(&state.pool, &state.user_id, &body.text).await?;
    Ok(Json(serde_json::json!({
        "charter": {
            "id": row.id,
            "text": row.text,
            "version": row.version,
            "updated_at": row.updated_at,
        }
    })))
}
