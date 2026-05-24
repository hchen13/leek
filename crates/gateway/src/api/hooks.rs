//! `GET /api/v1/hooks` — describe the effective hook configuration.
//! Read-only; install / update is `~/.leek/config.json` only.
//!
//! Commands are returned verbatim — the gateway is local-only and we
//! don't expose the API publicly, so there's no obfuscation step. Down
//! the road we may add a `redact_commands=true` query flag if remote
//! UIs ever ship.

use axum::extract::State;
use axum::Json;

use super::{ApiResult, AppState};

/// Shape the engine's `HookSet` into a JSON object the frontend can
/// render in Settings.
pub async fn list(State(st): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let set = st.hooks.set();
    let mut events = Vec::new();
    for (event, matchers) in &set.by_event {
        let mut entries = Vec::new();
        for m in matchers {
            let hooks: Vec<_> = m
                .hooks
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "type": h.kind,
                        "command": h.command,
                        "timeout_secs": h.timeout_secs,
                    })
                })
                .collect();
            entries.push(serde_json::json!({
                "matcher": m.matcher,
                "hooks": hooks,
            }));
        }
        events.push(serde_json::json!({
            "event": event.as_str(),
            "entries": entries,
        }));
    }
    // Sort by event name for stable output.
    events.sort_by(|a, b| {
        a["event"]
            .as_str()
            .cmp(&b["event"].as_str())
    });
    Ok(Json(serde_json::json!({ "events": events })))
}
