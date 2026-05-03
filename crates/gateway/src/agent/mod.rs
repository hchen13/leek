//! Agent pipeline — multi-turn loop over an LLM provider, dispatching
//! client-side function tools through `tools::ToolRegistry` and re-feeding
//! their outputs into the next turn until the model produces a terminal
//! `MessageEnd`. Server-side tools (codex `web_search`) are advertised in
//! the same `tools` array but the model executes them remotely; we only
//! surface lifecycle events for the UI.

pub mod compact;
pub mod harness;
pub mod routing;
pub mod tools;

use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role, ToolSpec, WebSearchAction};
use crate::vault::{self, events as vault_events, messages as vault_messages, tasks as vault_tasks};

use tools::{ToolContext, ToolRegistry};

const DEFAULT_MODEL: &str = "gpt-5.5";

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
    mandate_path: Option<std::path::PathBuf>,
) -> Result<()> {
    // Load full history. The hard cap is high enough that a session
    // shouldn't bump it before compaction kicks in; once compacted, the new
    // session starts at seq=1 again so the cap is fresh. Token-budget-based
    // trimming is replaced by /compact (#137) — when context bites, fork
    // rather than truncate.
    let history = vault_messages::list(&pool, &user_id, &session_id, None, 1000).await?;

    // Compaction summary rows (role=compaction_summary) get prepended to the
    // system prompt instead of going into the message list — they're "what
    // this session inherits from its parent", not turns the model said.
    let mut handoff_summaries: Vec<String> = Vec::new();
    let messages: Vec<ChatMessage> = history
        .iter()
        .filter_map(|row| {
            let content: serde_json::Value = serde_json::from_str(&row.content_json).ok()?;
            let text = content.get("text")?.as_str()?.to_string();
            let role = match row.role.as_str() {
                "user" => Role::User,
                "agent" => Role::Assistant,
                "compaction_summary" => {
                    handoff_summaries.push(text);
                    return None;
                }
                _ => return None, // skip other system / tool rows in this slice
            };
            Some(ChatMessage { role, content: text })
        })
        .collect();

    if messages.is_empty() && handoff_summaries.is_empty() {
        anyhow::bail!("run_chat_reply called with no user messages in session");
    }

    // Re-read mandate.md every turn so user edits take effect without
    // restart. Filesystem cache makes this near-free; a missing or empty
    // file omits the mandate section.
    let mandate_text = mandate_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok());
    let charter_text = vault::charters::get_active_text(&pool, &user_id)
        .await
        .unwrap_or(None);
    let system_prompt = harness::build_system_prompt(
        &handoff_summaries,
        mandate_text.as_deref(),
        charter_text.as_deref(),
    );

    let ctx = ToolContext {
        pool: pool.clone(),
        event_bus: event_bus.clone(),
        user_id: user_id.clone(),
        session_id: session_id.clone(),
        task_id: task.as_ref().map(|t| t.task_id.clone()),
    };

    // Build the tools array once: server-side web_search + every client-side
    // function tool registered in the registry. The model picks between them
    // based on each tool's `description` field (see tools/*.rs); cross-tool
    // discipline lives in `harness/discipline.md` §7. Set
    // LEEK_DISABLE_WEB_SEARCH=1 to force client-side-only tooling (useful for
    // diagnosing function_call dispatch in isolation).
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
    // Length of `full_text` at the start of the current turn — used to
    // slice out the text the model produced *during* this turn so we can
    // distinguish narration (preceded a tool call) from the final reply
    // (the last turn that had no pending tool calls).
    let mut turn_text_anchor = 0usize;

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
            system: Some(system_prompt.clone()),
            model: DEFAULT_MODEL.to_string(),
            max_output_tokens: None,
            tools: tool_specs.clone(),
            additional_inputs: additional_inputs.clone(),
            reasoning_effort: None,
        };

        let mut stream = provider.chat(req).await?;
        let mut pending_calls: Vec<PendingCall> = Vec::new();
        turn_text_anchor = full_text.chars().count();

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

        // Carve out the text the model produced *during this turn* — that's
        // narration (it preceded a tool dispatch) rather than the final
        // answer. We surface it as a separate event so the canvas can show
        // "agent's reasoning" alongside the artifacts. The text already
        // streamed into agent_message_delta above; we just attach a
        // structured boundary marker on top.
        let narration: String = full_text.chars().skip(turn_text_anchor).collect();
        let narration_trimmed = narration.trim();
        if !narration_trimmed.is_empty() {
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "agent_narration",
                serde_json::json!({
                    "turn": turn,
                    "text": narration_trimmed,
                }),
            )
            .await?;
        }

        // Execute pending tools sequentially (parallelism can come later;
        // most tools we'll ship are I/O bound so order rarely matters but
        // serializing keeps the audit trail simple).
        for call in pending_calls {
            let exec_result = tools
                .dispatch(&call.name, &call.arguments, cancel.clone(), &ctx)
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
                    "output_preview": preview(&output_str, 2000),
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
