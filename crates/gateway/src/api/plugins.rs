//! `GET /api/v1/plugins` — read-only list of loaded plugins. Failures at
//! load time go to the gateway log, not the API (the user wouldn't see
//! them via the workbench, but they'd find them in the logs when
//! debugging a plugin).
//!
//! For the MVP, what's loaded is derived at startup. Re-reading on every
//! request would let us hot-detect new plugins, but the load step is
//! moderately heavy and best done once — `M2.5+ hot reload` is a
//! separate item.

use axum::extract::State;
use axum::Json;

use super::{ApiResult, AppState};

/// Walk the same plugin roots used at startup so the API reports what is
/// *currently* on disk, not the snapshot from when the gateway booted.
/// This keeps the request synchronous and avoids needing to store the
/// loaded-plugin list on `AppState`.
pub async fn list(State(_st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let roots = vec![
        crate::skills::expand_home("~/.leek/plugins/"),
        std::path::PathBuf::from(".leek/plugins"),
    ];
    let (loaded, errors) = crate::plugins::load_all(&roots);
    let items: Vec<_> = loaded
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "description": p.description,
                "version": p.version,
                "author": p.author_display,
                "root": p.root.display().to_string(),
                "has_skills": p.skills_dir.is_some(),
                "has_hooks": p.hooks_file.is_some(),
            })
        })
        .collect();
    let errs: Vec<_> = errors.iter().map(|e| e.to_string()).collect();
    Ok(Json(serde_json::json!({
        "items": items,
        "load_errors": errs,
    })))
}
