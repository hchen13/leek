use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use super::{AppError, AppState};
use crate::vault::holdings as vault_holdings;
use crate::vault::holdings::{HoldingRow, NewHolding};

#[derive(Serialize)]
pub struct GetResponse {
    pub items: Vec<HoldingRow>,
    pub snapshot_at: Option<String>,
}

pub async fn get_handler(State(state): State<AppState>) -> Result<Json<GetResponse>, AppError> {
    let items = vault_holdings::get_latest(&state.pool, &state.user_id).await?;
    let snapshot_at = items.first().map(|r| r.snapshot_at.clone());
    Ok(Json(GetResponse { items, snapshot_at }))
}

#[derive(Deserialize)]
pub struct PostBody {
    pub holdings: Vec<NewHolding>,
}

#[derive(Serialize)]
pub struct PostResponse {
    pub snapshot_at: String,
}

pub async fn post_handler(
    State(state): State<AppState>,
    Json(body): Json<PostBody>,
) -> Result<Json<PostResponse>, AppError> {
    let snapshot_at =
        vault_holdings::insert_snapshot(&state.pool, &state.user_id, &body.holdings).await?;
    Ok(Json(PostResponse { snapshot_at }))
}

#[derive(Serialize)]
pub struct HistoryResponse {
    pub snapshots: Vec<String>,
}

pub async fn history_handler(
    State(state): State<AppState>,
) -> Result<Json<HistoryResponse>, AppError> {
    let snapshots = vault_holdings::list_snapshots(&state.pool, &state.user_id, 50).await?;
    Ok(Json(HistoryResponse { snapshots }))
}
