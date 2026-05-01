//! Agent pipeline — for the chat_reply slice this is just "call provider,
//! stream deltas to EventBus, persist final message". When the routing layer
//! and tool dispatch land, this module grows to host the full main agent loop.

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use sqlx::SqlitePool;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role};
use crate::vault::{events as vault_events, messages as vault_messages};

const DEFAULT_MODEL: &str = "gpt-5.5";
const SYSTEM_PROMPT: &str = "You are L.E.E.K — a helpful, concise \
investment-research assistant. Reply briefly in the user's language.";

/// Run a one-shot chat reply: invoke provider, stream events, persist final message.
///
/// All emitted events go to both `vault.events` (durable) and `event_bus`
/// (live SSE subscribers). User message is expected to already be persisted
/// by the caller (the POST handler).
pub async fn run_chat_reply(
    pool: SqlitePool,
    user_id: String,
    session_id: String,
    provider: Arc<dyn LlmProvider>,
    event_bus: EventBus,
    user_text: String,
) -> Result<()> {
    let req = ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: user_text,
        }],
        system: Some(SYSTEM_PROMPT.to_string()),
        model: DEFAULT_MODEL.to_string(),
        max_output_tokens: None,
    };

    publish_and_persist(
        &pool,
        &user_id,
        &session_id,
        &event_bus,
        "agent_message_start",
        serde_json::json!({}),
    )
    .await?;

    let mut stream = provider.chat(req).await?;
    let mut full_text = String::new();
    let mut stop_reason = "end_turn".to_string();

    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::TextDelta { text }) => {
                full_text.push_str(&text);
                publish_and_persist(
                    &pool,
                    &user_id,
                    &session_id,
                    &event_bus,
                    "agent_message_delta",
                    serde_json::json!({ "text": text }),
                )
                .await?;
            }
            Ok(LlmEvent::Usage(u)) => {
                publish_and_persist(
                    &pool,
                    &user_id,
                    &session_id,
                    &event_bus,
                    "llm_usage",
                    serde_json::json!({
                        "provider": provider.name(),
                        "input_tokens": u.input_tokens,
                        "output_tokens": u.output_tokens,
                        "cache_read_tokens": u.cache_read_tokens,
                    }),
                )
                .await?;
            }
            Ok(LlmEvent::MessageEnd { stop_reason: sr }) => {
                stop_reason = format!("{sr:?}").to_lowercase();
            }
            Err(e) => {
                publish_and_persist(
                    &pool,
                    &user_id,
                    &session_id,
                    &event_bus,
                    "error",
                    serde_json::json!({ "message": e.to_string() }),
                )
                .await?;
                return Err(e);
            }
        }
    }

    let msg_seq = vault_messages::insert(
        &pool,
        &user_id,
        &session_id,
        "agent",
        &serde_json::json!({ "type": "text", "text": full_text }),
    )
    .await?;

    publish_and_persist(
        &pool,
        &user_id,
        &session_id,
        &event_bus,
        "agent_message_end",
        serde_json::json!({
            "stop_reason": stop_reason,
            "message_seq": msg_seq,
        }),
    )
    .await?;

    Ok(())
}

async fn publish_and_persist(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    event_bus: &EventBus,
    kind: &str,
    payload: serde_json::Value,
) -> Result<()> {
    let ts = chrono::Utc::now();
    let seq = vault_events::insert(
        pool,
        user_id,
        session_id,
        None,
        kind,
        &payload,
        Some("main_agent"),
        ts,
    )
    .await?;
    event_bus
        .publish(
            session_id,
            EventEnvelope {
                seq,
                kind: kind.to_string(),
                payload,
                ts,
            },
        )
        .await;
    Ok(())
}
