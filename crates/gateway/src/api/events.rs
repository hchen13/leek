//! Event endpoints — durable history + live SSE stream.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use super::{ApiResult, AppState};
use crate::vault::events;

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /api/v1/sessions/{id}/events?since=&limit=` — durable event history.
pub async fn history(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = events::list(&st.pool, &session_id, q.since, q.limit.unwrap_or(500)).await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// `GET /stream/sessions/{id}/events` — live SSE stream of the session's events.
pub async fn stream(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let mut rx = st.bus.subscribe(&session_id).await;
    let body = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event)
                        .unwrap_or_else(|_| "{}".to_string());
                    yield Ok::<_, Infallible>(
                        SseEvent::default().event(event.kind).data(data),
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(body).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
