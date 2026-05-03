//! HTTP / SSE API — axum router + shared `AppState`.

pub mod corpus;
pub mod health;
pub mod messages;
pub mod portfolio;
pub mod sessions;
pub mod static_files;
pub mod stream;

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::{Any, CorsLayer};

use crate::agent::tools::ToolRegistry;
use crate::events::EventBus;
use crate::llm::LlmProvider;

/// One in-flight per-session piece of work — either a chat reply, a manual
/// `/compact`, or an auto pre-turn compaction. The `user_cancellable` flag
/// gates `POST /abort`: chat replies and manual compactions are cancellable
/// (Esc / `/compact` chip stop button); auto compactions are *not* —
/// the user is forced to wait because the alternative is the next turn
/// blowing past the model's context window.
#[derive(Clone)]
pub struct ActiveTask {
    pub token: CancellationToken,
    pub user_cancellable: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub provider: Arc<dyn LlmProvider>,
    pub event_bus: EventBus,
    pub user_id: String,
    /// Per-session active task (chat reply / manual compact / auto compact).
    /// Inserted by `messages::post_handler` and `sessions::start_compaction`,
    /// cancelled by `sessions::abort_handler` (subject to `user_cancellable`)
    /// or removed by background-task cleanup once finished.
    pub active_replies: Arc<Mutex<HashMap<String, ActiveTask>>>,
    /// Client-side tool registry passed into the agent loop on each reply.
    pub tools: ToolRegistry,
    /// Filesystem path to the active user's mandate.md file (their investment
    /// preferences / risk tolerance / position caps / etc — discipline §5).
    /// Resolved at boot from the vault directory: `<vault-dir>/mandates/<user_id>.md`.
    /// Read on every chat turn so edits take effect immediately. `None` only
    /// in degenerate test setups where the path can't be resolved.
    pub mandate_path: Option<std::path::PathBuf>,
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any);

    Router::new()
        .route("/api/v1/health", get(health::handler))
        .route("/api/v1/corpus/graph", get(corpus::graph_handler))
        .route("/api/v1/corpus/doc", get(corpus::doc_handler))
        .route(
            "/api/v1/sessions",
            get(sessions::list_handler).post(sessions::create_handler),
        )
        .route(
            "/api/v1/sessions/{id}",
            patch(sessions::patch_handler).delete(sessions::delete_handler),
        )
        .route(
            "/api/v1/sessions/{id}/messages",
            post(messages::post_handler).get(messages::list_handler),
        )
        .route(
            "/api/v1/sessions/{id}/abort",
            post(sessions::abort_handler),
        )
        .route(
            "/api/v1/sessions/{id}/compact",
            post(sessions::compact_handler),
        )
        .route(
            "/api/v1/sessions/{id}/events",
            get(sessions::events_handler),
        )
        .route("/stream/sessions/{id}/events", get(stream::handler))
        .route(
            "/api/v1/portfolio",
            get(portfolio::get_handler).post(portfolio::post_handler),
        )
        .route("/api/v1/portfolio/history", get(portfolio::history_handler))
        .with_state(state)
        // Anything not matched above falls through to the embedded SPA
        .fallback(static_files::handler)
        .layer(cors)
}

/// Bridge `anyhow::Error` to a JSON 500 response.
pub struct AppError(pub anyhow::Error);

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        tracing::error!(error = %self.0, "request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "code": "INTERNAL_ERROR",
                    "message": self.0.to_string(),
                }
            })),
        )
            .into_response()
    }
}
