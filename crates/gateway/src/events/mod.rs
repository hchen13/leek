//! Event envelope + per-session broadcast bus.
//!
//! Each session has its own `tokio::broadcast::Sender<EventEnvelope>` lazily
//! created on first publish/subscribe. Subscribers (SSE / WS) receive a fan-out
//! of every event published in that session. Persistence to `vault.events` is
//! a separate path (publishers also write to vault directly).

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{broadcast, RwLock};

/// Capacity per-session — if a slow subscriber falls more than this behind,
/// it gets `Lagged` errors and recovers via Last-Event-ID replay (P2).
const CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize)]
pub struct EventEnvelope {
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub ts: chrono::DateTime<chrono::Utc>,
}

impl EventEnvelope {
    pub fn new(seq: i64, kind: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            seq,
            kind: kind.into(),
            payload,
            ts: chrono::Utc::now(),
        }
    }
}

#[derive(Clone, Default)]
pub struct EventBus {
    inner: Arc<RwLock<HashMap<String, broadcast::Sender<EventEnvelope>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    async fn channel(&self, session_id: &str) -> broadcast::Sender<EventEnvelope> {
        if let Some(tx) = self.inner.read().await.get(session_id) {
            return tx.clone();
        }
        let mut w = self.inner.write().await;
        w.entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }

    pub async fn subscribe(&self, session_id: &str) -> broadcast::Receiver<EventEnvelope> {
        self.channel(session_id).await.subscribe()
    }

    /// Publish event. If no subscribers, send fails silently — vault.events
    /// is the durable record (subscriber catches up via Last-Event-ID replay).
    pub async fn publish(&self, session_id: &str, evt: EventEnvelope) {
        let tx = self.channel(session_id).await;
        let _ = tx.send(evt);
    }
}
