//! Agent pipeline — call provider, stream deltas to EventBus, persist final
//! message. When tool dispatch lands, this module grows to host the full main
//! agent loop.

pub mod routing;

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use sqlx::SqlitePool;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role};
use crate::vault::{events as vault_events, messages as vault_messages, tasks as vault_tasks};

const DEFAULT_MODEL: &str = "gpt-5.5";
const SYSTEM_PROMPT: &str = "You are L.E.E.K — a helpful, concise \
investment-research assistant. Reply briefly in the user's language.";

/// When set, the agent's reply is treated as the deliverable for that task —
/// vault.deliverables row is written and the task is marked delivered.
#[derive(Debug, Clone)]
pub struct TaskBinding {
    pub task_id: String,
    pub expected_deliverable: String,
}

/// Run a one-shot chat reply: invoke provider with full session history,
/// stream events, persist final message.
///
/// All emitted events go to both `vault.events` (durable) and `event_bus`
/// (live SSE subscribers). The triggering user message is expected to already
/// be persisted by the caller (the POST handler) — we read it back from vault
/// as part of the message history, so multi-turn context flows naturally.
pub async fn run_chat_reply(
    pool: SqlitePool,
    user_id: String,
    session_id: String,
    provider: Arc<dyn LlmProvider>,
    event_bus: EventBus,
    task: Option<TaskBinding>,
) -> Result<()> {
    // Load full history. P1: cap at last 100 messages — context-trimming logic
    // (data-schema.md / agent-loop.md §4.3) lands when token budgets matter.
    let history = vault_messages::list(&pool, &user_id, &session_id, None, 100).await?;

    let messages: Vec<ChatMessage> = history
        .iter()
        .filter_map(|row| {
            let content: serde_json::Value = serde_json::from_str(&row.content_json).ok()?;
            let text = content.get("text")?.as_str()?.to_string();
            let role = match row.role.as_str() {
                "user" => Role::User,
                "agent" => Role::Assistant,
                _ => return None, // skip system / tool rows in this slice
            };
            Some(ChatMessage { role, content: text })
        })
        .collect();

    if messages.is_empty() {
        anyhow::bail!("run_chat_reply called with no user messages in session");
    }

    let req = ChatRequest {
        messages,
        system: Some(SYSTEM_PROMPT.to_string()),
        model: DEFAULT_MODEL.to_string(),
        max_output_tokens: None,
    };

    publish_and_persist(
        &pool,
        &user_id,
        &session_id,
        task.as_ref().map(|t| t.task_id.as_str()),
        &event_bus,
        "agent_message_start",
        serde_json::json!({ "task_id": task.as_ref().map(|t| &t.task_id) }),
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
                    task.as_ref().map(|t| t.task_id.as_str()),
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
                    task.as_ref().map(|t| t.task_id.as_str()),
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
                    task.as_ref().map(|t| t.task_id.as_str()),
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
        task.as_ref().map(|t| t.task_id.as_str()),
    )
    .await?;

    // If this run was bound to a task, write the deliverable + mark delivered
    // before announcing message_end so subscribers see a coherent terminal state.
    if let Some(t) = task.as_ref() {
        let deliverable_id = vault_tasks::write_deliverable(
            &pool,
            &user_id,
            &t.task_id,
            &t.expected_deliverable,
            &full_text,
        )
        .await?;
        vault_tasks::mark_delivered(&pool, &user_id, &t.task_id).await?;

        publish_and_persist(
            &pool,
            &user_id,
            &session_id,
            Some(&t.task_id),
            &event_bus,
            "deliverable_ready",
            serde_json::json!({
                "deliverable_id": deliverable_id,
                "task_id": t.task_id,
                "kind": t.expected_deliverable,
            }),
        )
        .await?;
        publish_and_persist(
            &pool,
            &user_id,
            &session_id,
            Some(&t.task_id),
            &event_bus,
            "task_delivered",
            serde_json::json!({ "task_id": t.task_id }),
        )
        .await?;
    }

    publish_and_persist(
        &pool,
        &user_id,
        &session_id,
        task.as_ref().map(|t| t.task_id.as_str()),
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

pub async fn publish_and_persist(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    kind: &str,
    payload: serde_json::Value,
) -> Result<()> {
    let ts = chrono::Utc::now();
    let seq = vault_events::insert(
        pool,
        user_id,
        session_id,
        task_id,
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
