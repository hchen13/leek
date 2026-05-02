//! Agent pipeline — multi-turn loop over an LLM provider, dispatching
//! client-side function tools through `tools::ToolRegistry` and re-feeding
//! their outputs into the next turn until the model produces a terminal
//! `MessageEnd`. Server-side tools (codex `web_search`) are advertised in
//! the same `tools` array but the model executes them remotely; we only
//! surface lifecycle events for the UI.

pub mod routing;
pub mod tools;

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role, ToolSpec, WebSearchAction};
use crate::vault::{events as vault_events, messages as vault_messages, tasks as vault_tasks};

use tools::ToolRegistry;

const DEFAULT_MODEL: &str = "gpt-5.5";
const SYSTEM_PROMPT: &str = "You are L.E.E.K — a helpful, concise \
investment-research assistant. Reply briefly in the user's language.\n\n\
TOOL USE — you have two complementary web tools, pick the right one:\n\
1. `web_search` (built-in) — quick discovery / fresh facts / live quotes / \
   news headlines. Returns search results + short snippets. Use this when \
   you need to FIND something or check a small fact.\n\
2. `web_fetch` (function tool) — open and READ a specific known URL in \
   full, returned as clean markdown (Readability-extracted). Use this \
   when the user gives you a URL to read, when you need to extract data \
   from a filing / earnings release / blog post / PR, or when web_search \
   snippets aren't enough and you need the full article body.\n\
Whenever the user supplies a URL or you need full-page content (not just \
a summary), prefer `web_fetch` over `web_search`. Always cite source URLs \
in your final answer.";

/// Hard cap on tool-call rounds within a single user turn. Prevents runaway
/// loops where the model keeps re-invoking tools without reaching a final
/// answer. 8 turns covers fan-out research (search → open 3 pages → re-search)
/// comfortably; anything beyond is almost certainly a bug or prompt issue.
const MAX_TOOL_TURNS: usize = 8;

/// When set, the agent's reply is treated as the deliverable for that task —
/// vault.deliverables row is written and the task is marked delivered.
#[derive(Debug, Clone)]
pub struct TaskBinding {
    pub task_id: String,
    pub expected_deliverable: String,
}

struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Truncate a string at byte boundary (UTF-8 safe) for SSE preview payloads.
fn preview(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Run a one-shot chat reply: invoke provider with full session history,
/// stream events, persist final message.
///
/// All emitted events go to both `vault.events` (durable) and `event_bus`
/// (live SSE subscribers). The triggering user message is expected to already
/// be persisted by the caller (the POST handler) — we read it back from vault
/// as part of the message history, so multi-turn context flows naturally.
#[allow(clippy::too_many_arguments)]
pub async fn run_chat_reply(
    pool: SqlitePool,
    user_id: String,
    session_id: String,
    provider: Arc<dyn LlmProvider>,
    event_bus: EventBus,
    task: Option<TaskBinding>,
    cancel: CancellationToken,
    tools: ToolRegistry,
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

    // Build the tools array once: server-side web_search + every client-side
    // function tool registered in the registry. The model picks between them
    // based on the description text — we steer toward web_fetch for "read this
    // URL" cases via SYSTEM_PROMPT, and let web_search handle discovery / live
    // facts. Set LEEK_DISABLE_WEB_SEARCH=1 to force client-side-only tooling
    // (useful for diagnosing function_call dispatch in isolation).
    let mut tool_specs: Vec<ToolSpec> = if std::env::var("LEEK_DISABLE_WEB_SEARCH").is_ok() {
        Vec::new()
    } else {
        vec![ToolSpec::WebSearch {
            external_web_access: true,
        }]
    };
    tool_specs.extend(tools.specs());

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

    let mut full_text = String::new();
    let mut stop_reason = "end_turn".to_string();
    let mut additional_inputs: Vec<serde_json::Value> = Vec::new();
    let mut turn = 0usize;

    'turns: loop {
        if turn >= MAX_TOOL_TURNS {
            stop_reason = "max_tool_turns".to_string();
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "error",
                serde_json::json!({
                    "message": format!("agent exceeded MAX_TOOL_TURNS ({MAX_TOOL_TURNS}); aborting"),
                }),
            )
            .await?;
            break 'turns;
        }

        let req = ChatRequest {
            messages: messages.clone(),
            system: Some(SYSTEM_PROMPT.to_string()),
            model: DEFAULT_MODEL.to_string(),
            max_output_tokens: None,
            tools: tool_specs.clone(),
            additional_inputs: additional_inputs.clone(),
        };

        let mut stream = provider.chat(req).await?;
        let mut pending_calls: Vec<PendingCall> = Vec::new();

        'stream: loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    stop_reason = "user_aborted".to_string();
                    break 'turns;
                }
                evt_opt = stream.next() => {
                    let Some(event) = evt_opt else { break 'stream };
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
                        Ok(LlmEvent::WebSearchCall { status, action }) => {
                            let (action_kind, action_detail) = match &action {
                                Some(WebSearchAction::Search { query }) => ("search", query.clone()),
                                Some(WebSearchAction::OpenPage { url }) => ("open_page", url.clone()),
                                Some(WebSearchAction::FindInPage { url, pattern }) => {
                                    ("find_in_page", format!("{pattern} @ {url}"))
                                }
                                Some(WebSearchAction::Other) => ("other", String::new()),
                                None => ("unknown", String::new()),
                            };
                            publish_and_persist(
                                &pool,
                                &user_id,
                                &session_id,
                                task.as_ref().map(|t| t.task_id.as_str()),
                                &event_bus,
                                "web_search_call",
                                serde_json::json!({
                                    "status": status,
                                    "action": action_kind,
                                    "detail": action_detail,
                                }),
                            )
                            .await?;
                        }
                        Ok(LlmEvent::FunctionCall { call_id, name, arguments }) => {
                            // Surface "tool starting" to the UI before we
                            // actually execute (gives a chip immediately even
                            // for slow tools like web_fetch).
                            publish_and_persist(
                                &pool,
                                &user_id,
                                &session_id,
                                task.as_ref().map(|t| t.task_id.as_str()),
                                &event_bus,
                                "tool_call",
                                serde_json::json!({
                                    "status": "in_progress",
                                    "call_id": call_id,
                                    "name": name,
                                    "arguments": arguments,
                                }),
                            )
                            .await?;
                            pending_calls.push(PendingCall { call_id, name, arguments });
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
            }
        }

        // No tool calls this turn → model is done.
        if pending_calls.is_empty() {
            break 'turns;
        }

        // Execute pending tools sequentially (parallelism can come later;
        // most tools we'll ship are I/O bound so order rarely matters but
        // serializing keeps the audit trail simple).
        for call in pending_calls {
            let exec_result = tools
                .dispatch(&call.name, &call.arguments, cancel.clone())
                .await;

            // Treat tool errors as a delivered output: the model sees the
            // error string and decides what to do (retry / give up / keep
            // going). We do NOT propagate as Err — that would kill the turn.
            let (output_str, status) = match exec_result {
                Ok(s) => (s, "completed"),
                Err(e) => (format!("[tool error: {e}]"), "error"),
            };

            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "tool_call",
                serde_json::json!({
                    "status": status,
                    "call_id": call.call_id,
                    "name": call.name,
                    // Truncate output preview for SSE / UI; full output still
                    // goes to the model via additional_inputs below.
                    "output_preview": preview(&output_str, 240),
                    "output_bytes": output_str.len(),
                }),
            )
            .await?;

            // Echo the assistant's function_call back into the input stream
            // (codex requires this for the model to "see" its own call), then
            // append our function_call_output. Order matters: the call must
            // precede its output.
            additional_inputs.push(serde_json::json!({
                "type": "function_call",
                "call_id": call.call_id,
                "name": call.name,
                "arguments": call.arguments,
            }));
            additional_inputs.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call.call_id,
                "output": output_str,
            }));
        }

        turn += 1;
    }

    let has_content = !full_text.is_empty();

    // Only persist a message + deliverable when there's actually content. An
    // abort that fires before the first delta produces an empty `full_text`
    // — we treat that as a clean cancel: no orphan empty message, no empty
    // deliverable. The task is still marked delivered (no `cancelled` state
    // by design — see memory:feedback_stop_is_stream_abort).
    let msg_seq = if has_content {
        Some(
            vault_messages::insert(
                &pool,
                &user_id,
                &session_id,
                "agent",
                &serde_json::json!({ "type": "text", "text": full_text }),
                task.as_ref().map(|t| t.task_id.as_str()),
            )
            .await?,
        )
    } else {
        None
    };

    if let Some(t) = task.as_ref() {
        if has_content {
            let deliverable_id = vault_tasks::write_deliverable(
                &pool,
                &user_id,
                &t.task_id,
                &t.expected_deliverable,
                &full_text,
            )
            .await?;
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
        }
        vault_tasks::mark_delivered(&pool, &user_id, &t.task_id).await?;
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
