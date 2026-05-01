//! HTTP / SSE API — axum router + shared `AppState`.

pub mod health;
pub mod messages;
pub mod static_files;
pub mod stream;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::cors::{Any, CorsLayer};

use crate::events::EventBus;
use crate::llm::LlmProvider;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::SqlitePool,
    pub provider: Arc<dyn LlmProvider>,
    pub event_bus: EventBus,
    pub user_id: String,
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_methods(Any);

    Router::new()
        .route("/api/v1/health", get(health::handler))
        .route(
            "/api/v1/sessions/{id}/messages",
            post(messages::post_handler).get(messages::list_handler),
        )
        .route("/stream/sessions/{id}/events", get(stream::handler))
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
