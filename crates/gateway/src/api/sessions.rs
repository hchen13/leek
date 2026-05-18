//! Session endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{ApiError, ApiResult, AppState};
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
pub async fn remove(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if sessions::delete(&st.pool, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("session '{id}' not found")))
    }
}
