//! Per-session SSE broadcast bus.
//!
//! Each session has its own `tokio::broadcast` channel, created lazily on the
//! first publish or subscribe. Persistence is separate — `vault::events` is
//! the durable record; the bus only fans out to live subscribers.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use crate::vault::events::Event;

const CHANNEL_CAPACITY: usize = 1024;

#[derive(Clone, Default)]
pub struct EventBus {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<Event>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    async fn channel(&self, session_id: &str) -> broadcast::Sender<Event> {
        if let Some(tx) = self.channels.read().await.get(session_id) {
            return tx.clone();
        }
        self.channels
            .write()
            .await
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }

    /// Subscribe to a session's live event stream.
    pub async fn subscribe(&self, session_id: &str) -> broadcast::Receiver<Event> {
        self.channel(session_id).await.subscribe()
    }

    /// Fan an event out to live subscribers. A send with no subscribers fails
    /// silently — the `events` table is the durable record.
    pub async fn publish(&self, session_id: &str, event: Event) {
        let _ = self.channel(session_id).await.send(event);
    }
}
