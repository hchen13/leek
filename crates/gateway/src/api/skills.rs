//! `GET /api/v1/skills` and `GET /api/v1/skills/{name}` — read-only view
//! of the loaded skill registry. The workbench shows the index in
//! Settings; `GET /{name}` lets the UI preview a body before the model
//! chooses to invoke it.

use axum::extract::{Path, State};
use axum::Json;

use super::{ApiError, ApiResult, AppState};

/// `GET /api/v1/skills` — names + descriptions + source layer for every
/// skill in the registry (model-hidden ones included; the frontend can
/// decide whether to surface them).
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<_> = st
        .skills
        .iter_sorted()
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "source_layer": s.source_layer.as_str(),
                "allowed_tools": s.allowed_tools,
                "disable_model_invocation": s.disable_model_invocation,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// `GET /api/v1/skills/{name}` — full skill metadata including body.
/// Returns 404 if the skill is not registered.
pub async fn detail(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(s) = st.skills.get(&name) else {
        return Err(ApiError::not_found(format!("skill '{name}' not found")));
    };
    Ok(Json(serde_json::json!({
        "name": s.name,
        "description": s.description,
        "source_layer": s.source_layer.as_str(),
        "allowed_tools": s.allowed_tools,
        "disable_model_invocation": s.disable_model_invocation,
        "body": s.body.as_ref(),
    })))
}
