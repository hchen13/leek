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
        // Track the most recent seq we've actually emitted to this client so
        // the lag payload tells the frontend exactly where to resume from.
        let mut last_seq: i64 = 0;
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let data = serde_json::to_string(&evt.payload)
                        .unwrap_or_else(|_| "{}".into());
                    last_seq = evt.seq;
                    let sse = SseEvent::default()
                        .id(evt.seq.to_string())
                        .event(evt.kind.clone())
                        .data(data);
                    yield Ok::<SseEvent, Infallible>(sse);
                }
                Err(RecvError::Lagged(n)) => {
                    // Surface a structured `stream_lag` event so the client
                    // can backfill via GET /events?since=<last_seq>. The old
                    // free-text warning was undecodable by the JSON parser.
                    let payload = serde_json::json!({
                        "missed": n,
                        "last_seq": last_seq,
                    });
                    let data = serde_json::to_string(&payload)
                        .unwrap_or_else(|_| "{}".into());
                    let sse = SseEvent::default().event("stream_lag").data(data);
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
