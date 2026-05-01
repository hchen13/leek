//! SSE event stream — subscribers receive every event published to the bus
//! for a given session.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use tokio::sync::broadcast::error::RecvError;

use super::AppState;

pub async fn handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = state.event_bus.subscribe(&session_id).await;

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let data = serde_json::to_string(&evt.payload)
                        .unwrap_or_else(|_| "{}".into());
                    let sse = SseEvent::default()
                        .id(evt.seq.to_string())
                        .event(evt.kind.clone())
                        .data(data);
                    yield Ok::<SseEvent, Infallible>(sse);
                }
                Err(RecvError::Lagged(n)) => {
                    let sse = SseEvent::default()
                        .event("warning")
                        .data(format!("subscriber lagged behind {n} events"));
                    yield Ok(sse);
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("heartbeat"),
    )
}
