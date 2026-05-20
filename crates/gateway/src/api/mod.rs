//! HTTP / SSE API — Axum router, shared state, error type.

pub mod events;
pub mod health;
pub mod messages;
pub mod sessions;
pub mod transcripts;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use tower_http::cors::{Any, CorsLayer};

use crate::agent::GuardConfig;
use crate::bus::EventBus;
use crate::llm::codex::CodexClient;
use crate::vault::events::Event;

/// Shared, cheaply-cloneable handles for request handlers and agent turns.
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub bus: EventBus,
    /// The codex backend client (model access).
    pub codex: CodexClient,
    /// HTTP client for client-side tools (the `web_fetch` tool).
    pub http: reqwest::Client,
    /// Agent-loop safety nets, resolved once at startup.
    pub guards: GuardConfig,
    /// Whether to offer the provider-side `web_search` tool (M1.9.4).
    /// Resolved from `LEEK_WEB_SEARCH` at startup; off by default.
    pub web_search: bool,
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
