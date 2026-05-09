//! GET / PATCH /api/v1/settings — surfaces the per-surface LLM tuning so the
//! frontend Settings page can render dropdowns and persist changes.
//!
//! On PATCH, we (1) write to `vault.user_settings`, (2) hot-swap the in-memory
//! tuning in `AppState` so the next chat turn picks up the new values without
//! a server restart.

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::llm::{LlmTuning, ReasoningEffort, SurfaceTuning, Verbosity};
use crate::vault::user_settings;

use super::{AppError, AppState};

fn surface_to_json(s: SurfaceTuning) -> serde_json::Value {
    serde_json::json!({
        "reasoning_effort": s.reasoning_effort.as_str(),
        "verbosity": s.verbosity.as_str(),
    })
}

fn tuning_to_json(t: LlmTuning) -> serde_json::Value {
    serde_json::json!({
        "main": surface_to_json(t.main),
        "routing": surface_to_json(t.routing),
        "compaction": surface_to_json(t.compaction),
        "subagent": surface_to_json(t.subagent),
    })
}

pub async fn get_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tuning = state.tuning_snapshot();
    Ok(Json(serde_json::json!({
        "tuning": tuning_to_json(tuning),
        "options": {
            "reasoning_effort": ["minimal", "low", "medium", "high", "xhigh"],
            "verbosity": ["low", "medium", "high"],
        },
    })))
}

#[derive(Deserialize)]
pub struct SurfacePatch {
    pub reasoning_effort: Option<String>,
    pub verbosity: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct PatchBody {
    pub main: Option<SurfacePatch>,
    pub routing: Option<SurfacePatch>,
    pub compaction: Option<SurfacePatch>,
    pub subagent: Option<SurfacePatch>,
}

fn apply_surface(
    base: SurfaceTuning,
    patch: Option<SurfacePatch>,
) -> Result<SurfaceTuning, String> {
    let Some(p) = patch else {
        return Ok(base);
    };
    let reasoning_effort = match p.reasoning_effort {
        Some(s) => ReasoningEffort::from_str_ci(&s)
            .ok_or_else(|| format!("invalid reasoning_effort: {s}"))?,
        None => base.reasoning_effort,
    };
    let verbosity = match p.verbosity {
        Some(s) => Verbosity::from_str_ci(&s).ok_or_else(|| format!("invalid verbosity: {s}"))?,
        None => base.verbosity,
    };
    Ok(SurfaceTuning {
        reasoning_effort,
        verbosity,
    })
}

pub async fn patch_handler(
    State(state): State<AppState>,
    Json(body): Json<PatchBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let current = state.tuning_snapshot();
    let next = LlmTuning {
        main: apply_surface(current.main, body.main).map_err(AppError::bad_request)?,
        routing: apply_surface(current.routing, body.routing).map_err(AppError::bad_request)?,
        compaction: apply_surface(current.compaction, body.compaction)
            .map_err(AppError::bad_request)?,
        subagent: apply_surface(current.subagent, body.subagent).map_err(AppError::bad_request)?,
        // Settings PATCH only touches surface tunings (reasoning_effort /
        // verbosity per surface). Guards have no UI surface yet — they're
        // wired through M1.2-1.6 and exposed once the per-user override
        // schema in user_settings supports them. For now, preserve the
        // currently active GuardConfig.
        guards: current.guards,
    };

    user_settings::save_tuning(&state.pool, &state.user_id, next).await?;
    {
        let mut guard = state.tuning.write().expect("tuning lock poisoned");
        *guard = next;
    }

    Ok(Json(serde_json::json!({
        "tuning": tuning_to_json(next),
    })))
}
