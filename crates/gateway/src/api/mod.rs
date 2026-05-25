//! HTTP / SSE API — Axum router, shared state, error type.

pub mod agents;
pub mod corpus;
pub mod events;
pub mod health;
pub mod hooks;
pub mod messages;
pub mod plugins;
pub mod sessions;
pub mod settings;
pub mod skills;
pub mod transcripts;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use tower_http::cors::{Any, CorsLayer};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tokio::sync::Notify;

use crate::agents::AgentRegistry;
use crate::bus::EventBus;
use crate::config::Config;
use crate::corpus::{Corpus, CorpusGraph};
use crate::hooks::HookEngine;
use crate::llm::codex::CodexClient;
use crate::skills::SkillRegistry;
use crate::vault::events::Event;
use crate::vendors::VendorRegistry;

/// M3.2: per-turn user-abort registry. The agent loop registers an
/// `Arc<Notify>` keyed by `turn_id` on entry and drops the entry on
/// exit; the `POST .../abort` endpoint looks it up and calls
/// `notify_one()` to wake the loop. A separate `Arc` is held by the
/// loop itself (so a `remove()` racing with `notify_one()` cannot
/// invalidate the wake). `RwLock` (std) is fine — both ops are
/// short and never await with the guard held.
pub type AbortRegistry = Arc<RwLock<HashMap<String, Arc<Notify>>>>;

/// Shared, cheaply-cloneable handles for request handlers and agent turns.
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub bus: EventBus,
    /// The codex backend client (model access).
    pub codex: CodexClient,
    /// HTTP client for client-side tools (the `web_fetch` tool).
    pub http: reqwest::Client,
    /// Live user settings (M2.6). `Arc<RwLock<Config>>` so the settings
    /// API can mutate this once a PATCH lands and the next turn's
    /// `GuardConfig::resolve` picks up the new values — no restart.
    /// Read happens under a brief lock followed by a `.clone()`; never
    /// hold the guard across an `.await`.
    pub config: Arc<RwLock<Config>>,
    /// Whether to offer the provider-side `web_search` tool (M1.9.4).
    /// Resolved from `LEEK_WEB_SEARCH` at startup; off by default.
    pub web_search: bool,
    /// Read-only knowledge corpus (M2.1). Loaded once at startup, shared
    /// across requests via `Arc`. An empty corpus is a graceful-degrade
    /// path — the gateway boots without the knowledge layer rather than
    /// abort on a missing root.
    pub corpus: Arc<Corpus>,
    /// Precomputed wiki graph for the Corpus Brain UI (M2.1). Built once
    /// from `corpus` at startup; the corpus is not hot-reloaded so the
    /// graph is permanently fresh for the gateway's lifetime.
    pub corpus_graph: Arc<CorpusGraph>,
    /// Skill registry (M2.5): three-layer merged set of `SKILL.md` files
    /// available to this gateway. The system-prompt advertises model-visible
    /// skills; the model loads bodies on demand via the `use_skill` tool.
    pub skills: Arc<SkillRegistry>,
    /// Hook engine (M2.5): user-installed shell hooks attached to agent
    /// lifecycle events. Triggered by the agent loop at well-defined points
    /// (PreToolUse, PostToolUse, etc.).
    pub hooks: Arc<HookEngine>,
    /// Subagent registry (M2.7): three-layer merged set of `AGENT.md`
    /// definitions. The `task` tool resolves an `agent_name` against this
    /// to spawn a child loop with the named agent's system prompt and
    /// tool allow-list. Empty registry is fine — `task` is still
    /// advertised (with `general-purpose` as the default).
    pub agents: Arc<AgentRegistry>,
    /// Vendor registry (M3): upstream data clients (Tushare + Sina +
    /// EastMoney) used by the A-share research tools. Vendor identity is
    /// hidden from the model — see `vendors/` module docs.
    pub vendors: Arc<VendorRegistry>,
    /// M3.2: per-turn user-abort registry. See `AbortRegistry` docs.
    pub abort_signals: AbortRegistry,
}

impl AppState {
    /// Snapshot the current settings under a brief read lock. Callers
    /// (notably `agent::drive`) take this once at the start of a turn so
    /// the same values apply throughout that turn — a mid-turn PATCH only
    /// affects subsequent turns. A poisoned lock falls back to defaults
    /// rather than panicking so one bad write can't down the server.
    pub fn config_snapshot(&self) -> Config {
        match self.config.read() {
            Ok(g) => g.clone(),
            Err(poisoned) => {
                tracing::error!("settings RwLock poisoned; returning defaults for this read");
                poisoned.into_inner().clone()
            }
        }
    }
}

impl AppState {
    /// Persist an event to the durable log, then fan it out to live SSE
    /// subscribers. Use for meaningful, replayable lifecycle events.
    pub async fn emit(&self, session_id: &str, kind: &str, mut payload: serde_json::Value) {
        stamp_surface(&mut payload, kind);
        match crate::vault::events::insert(&self.pool, session_id, kind, &payload).await {
            Ok(event) => self.bus.publish(session_id, event).await,
            Err(e) => {
                tracing::error!(error = %e, session_id, kind, "failed to persist event");
            }
        }
    }

    /// Fan an event out to live subscribers WITHOUT persisting it. For
    /// high-frequency ephemeral events (assistant token deltas): the durable
    /// record of a turn is its `messages` row, not every streamed token.
    /// `seq = -1` marks the event as not coming from the durable log.
    pub async fn emit_ephemeral(&self, session_id: &str, kind: &str, mut payload: serde_json::Value) {
        stamp_surface(&mut payload, kind);
        let event = Event {
            seq: -1,
            kind: kind.to_string(),
            payload,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.bus.publish(session_id, event).await;
    }

    /// M3.2: register an abort-signal slot for a running turn. The agent
    /// loop calls this on entry and holds the returned `Arc<Notify>` so a
    /// concurrent `remove_abort_signal` can't drop the wake out from under
    /// it. A poisoned lock falls back to a fresh notify (the abort path is
    /// best-effort — the user can still ride out the turn the slow way).
    pub fn register_abort_signal(&self, turn_id: &str) -> Arc<Notify> {
        let notify = Arc::new(Notify::new());
        match self.abort_signals.write() {
            Ok(mut map) => {
                map.insert(turn_id.to_string(), notify.clone());
            }
            Err(poisoned) => {
                tracing::error!("abort_signals RwLock poisoned; abort for this turn disabled");
                let mut map = poisoned.into_inner();
                map.insert(turn_id.to_string(), notify.clone());
            }
        }
        notify
    }

    /// M3.2: drop the abort-signal slot when the turn exits. Called by the
    /// agent loop in its tail block so a turn that finished naturally is
    /// not abortable anymore (a stale `POST .../abort` then 404s — which is
    /// what the frontend wants too, since the pill is already gone).
    pub fn remove_abort_signal(&self, turn_id: &str) {
        if let Ok(mut map) = self.abort_signals.write() {
            map.remove(turn_id);
        }
    }

    /// M3.2: look up a turn's abort notifier. The endpoint calls this and
    /// fires `notify_one()` on a hit; a miss means the turn already exited
    /// (or was never registered) and the endpoint returns 404.
    pub fn lookup_abort_signal(&self, turn_id: &str) -> Option<Arc<Notify>> {
        self.abort_signals.read().ok()?.get(turn_id).cloned()
    }
}

/// Stamp an event payload with its workbench surface (M1.9.1), from the one
/// classification table in `agent::events` — the frontend reads the surface
/// off the event instead of hardcoding the map. A non-object payload is left
/// untouched (every gateway event payload is a JSON object).
fn stamp_surface(payload: &mut serde_json::Value, kind: &str) {
    if let serde_json::Value::Object(map) = payload {
        map.insert(
            "surface".to_string(),
            crate::agent::events::surface_for(kind).as_str().into(),
        );
    }
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any);

    Router::new()
        .route("/api/v1/health", get(health::handler))
        .route(
            "/api/v1/settings",
            get(settings::read).patch(settings::patch),
        )
        .route(
            "/api/v1/sessions",
            get(sessions::list).post(sessions::create),
        )
        .route(
            "/api/v1/sessions/{id}",
            patch(sessions::rename).delete(sessions::remove),
        )
        .route(
            "/api/v1/sessions/{id}/messages",
            get(messages::list).post(messages::post),
        )
        // M3.2: user-triggered abort of a running turn. Looks the turn's
        // notify slot up in `AppState::abort_signals`; 204 on a successful
        // wake, 404 if the turn has already exited.
        .route(
            "/api/v1/sessions/{id}/turns/{turn_id}/abort",
            post(sessions::abort_turn),
        )
        .route("/api/v1/sessions/{id}/events", get(events::history))
        .route("/stream/sessions/{id}/events", get(events::stream))
        // F2: raw LLM-layer archive (per-iteration request + SSE response).
        .route(
            "/api/v1/sessions/{id}/transcripts",
            get(transcripts::list),
        )
        .route(
            "/api/v1/sessions/{id}/transcripts/{turn_id}/{iteration}/request",
            get(transcripts::request_body),
        )
        .route(
            "/api/v1/sessions/{id}/transcripts/{turn_id}/{iteration}/response",
            get(transcripts::response_stream),
        )
        // M2.1: Corpus Brain — wiki graph + edge weights for the right-rail
        // visualisation. One-shot, no streaming; the graph is static for the
        // gateway's lifetime.
        .route("/api/v1/corpus/graph", get(corpus::graph))
        // M2.5 skill / hook / plugin read-only API.
        .route("/api/v1/skills", get(skills::list))
        .route("/api/v1/skills/{name}", get(skills::detail))
        .route("/api/v1/hooks", get(hooks::list))
        .route("/api/v1/plugins", get(plugins::list))
        // M2.7 subagent registry — read-only.
        .route("/api/v1/agents", get(agents::list))
        .route("/api/v1/agents/{name}", get(agents::detail))
        .layer(cors)
        .with_state(state)
}

/// Handler error type. `anyhow::Error` maps to 500; the explicit constructors
/// cover the 4xx cases.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    /// 400 with a free-form message. The settings PATCH handler also has a
    /// richer 400 path (`bad_request_with_fields`) for per-field errors.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(error = %err, "request failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
