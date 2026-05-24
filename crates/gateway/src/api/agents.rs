//! `GET /api/v1/agents` and `GET /api/v1/agents/{name}` — read-only view
//! of the loaded subagent registry. The workbench can use this for an
//! Agents settings panel (not built in M2.7 — user edits AGENT.md files);
//! the dispatch surface for spawning a subagent is the `task` tool.

use axum::extract::{Path, State};
use axum::Json;

use super::{ApiError, ApiResult, AppState};

/// `GET /api/v1/agents` — names + descriptions + source layer for every
/// agent in the registry.
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let items: Vec<_> = st
        .agents
        .iter_sorted()
        .into_iter()
        .map(|a| {
            serde_json::json!({
                "name": a.name,
                "description": a.description,
                "source_layer": a.source_layer.as_str(),
                "allowed_tools": a.allowed_tools,
                "model": a.model,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "items": items })))
}

/// `GET /api/v1/agents/{name}` — full agent metadata including the
/// system-prompt body. Returns 404 if the agent is not registered.
pub async fn detail(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(a) = st.agents.get(&name) else {
        return Err(ApiError::not_found(format!("agent '{name}' not found")));
    };
    Ok(Json(serde_json::json!({
        "name": a.name,
        "description": a.description,
        "source_layer": a.source_layer.as_str(),
        "allowed_tools": a.allowed_tools,
        "model": a.model,
        "system_prompt": a.system_prompt.as_ref(),
    })))
}
