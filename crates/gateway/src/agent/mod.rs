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
use std::time::Instant;

use anyhow::{anyhow, Result};
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{
    ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role, ToolSpec, WebSearchAction,
};
use crate::vault::{
    self, events as vault_events, messages as vault_messages, plans as vault_plans,
    tasks as vault_tasks, tool_runs as vault_tool_runs,
};

use tools::{ToolContext, ToolRegistry};

const DEFAULT_MODEL: &str = "gpt-5.5";

/// Hard cap on tool-call rounds within a single user turn.
const MAX_TOOL_TURNS: usize = 24;
const MAX_PROVIDER_RETRIES: usize = 10;
const PROVIDER_RETRY_BASE_MS: u64 = 1_000;
const PROVIDER_RETRY_MAX_MS: u64 = 30_000;
const PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 90_000;
const MAX_PLAN_GUARD_REWRITES: usize = 3;

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
pub(crate) fn preview(s: &str, max_bytes: usize) -> String {
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
    let all_history = vault_messages::list(&pool, &user_id, &session_id, None, 1000).await?;

    // Split at the last compaction_summary boundary. Pre-compaction rows stay
    // in the DB (shown read-only in the UI) but never enter LLM context.
    // The summary itself is injected into the system prompt as a handoff.
    let mut handoff_summaries: Vec<String> = Vec::new();
    let tail_start = all_history
        .iter()
        .rposition(|r| r.role == "compaction_summary")
        .map(|i| {
            if let Ok(c) = serde_json::from_str::<serde_json::Value>(&all_history[i].content_json) {
                if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                    handoff_summaries.push(t.to_string());
                }
            }
            i + 1
        })
        .unwrap_or(0);

    let messages: Vec<ChatMessage> = all_history[tail_start..]
        .iter()
        .filter_map(|row| {
            let content: serde_json::Value = serde_json::from_str(&row.content_json).ok()?;
            let text = content.get("text")?.as_str()?.to_string();
            let role = match row.role.as_str() {
                "user" => Role::User,
                "agent" => Role::Assistant,
                _ => return None,
            };
            Some(ChatMessage {
                role,
                content: text,
            })
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
    let mut final_text = String::new();
    let mut stop_reason = "end_turn".to_string();
    let mut additional_inputs: Vec<serde_json::Value> = Vec::new();
    let mut awaiting_user = false;
    let mut plan_guard_rewrites = 0usize;
    let mut turn = 0usize;
    let mut fatal_error: Option<String> = None;
    'turns: loop {
        if turn >= MAX_TOOL_TURNS {
            stop_reason = "max_tool_turns".to_string();
            let message = format!(
                "agent exceeded MAX_TOOL_TURNS ({MAX_TOOL_TURNS}); stopped before final response"
            );
            fatal_error = Some(message.clone());
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "error",
                serde_json::json!({
                    "message": message,
                }),
            )
            .await?;
            break 'turns;
        }

        let mut pending_calls: Vec<PendingCall> = Vec::new();
        let mut turn_text = String::new();
        let mut narration_buffer = String::new();
        let mut provider_retries = 0usize;

        loop {
            pending_calls.clear();
            turn_text.clear();
            narration_buffer.clear();
            let mut request_inputs = additional_inputs.clone();
            if let Some(plan_input) =
                active_plan_reminder_input(&pool, &user_id, &session_id, task.as_ref()).await?
            {
                request_inputs.push(plan_input);
            }
            let req = ChatRequest {
                messages: messages.clone(),
                system: Some(system_prompt.clone()),
                model: DEFAULT_MODEL.to_string(),
                max_output_tokens: None,
                tools: tool_specs.clone(),
                additional_inputs: request_inputs,
                reasoning_effort: None,
            };

            let mut stream = match provider.chat(req).await {
                Ok(stream) => stream,
                Err(e) => {
                    if provider_retries < MAX_PROVIDER_RETRIES {
                        provider_retries += 1;
                        let delay_ms = provider_retry_delay_ms(provider_retries);
                        publish_provider_retry(
                            &pool,
                            &user_id,
                            &session_id,
                            task.as_ref().map(|t| t.task_id.as_str()),
                            &event_bus,
                            provider.name(),
                            provider_retries,
                            delay_ms,
                            &e.to_string(),
                        )
                        .await?;
                        if !wait_retry(delay_ms, &cancel).await {
                            stop_reason = "user_aborted".to_string();
                            fatal_error = Some("user_aborted".to_string());
                            break 'turns;
                        }
                        continue;
                    }
                    publish_provider_error(
                        &pool,
                        &user_id,
                        &session_id,
                        task.as_ref().map(|t| t.task_id.as_str()),
                        &event_bus,
                        provider.name(),
                        &e.to_string(),
                    )
                    .await?;
                    if let Some(t) = task.as_ref() {
                        fail_task(
                            &pool,
                            &user_id,
                            &session_id,
                            &event_bus,
                            &t.task_id,
                            &format!("provider_error: {}", e),
                        )
                        .await?;
                    }
                    persist_partial_agent_message(
                        &pool,
                        &user_id,
                        &session_id,
                        &full_text,
                        task.as_ref().map(|t| t.task_id.as_str()),
                    )
                    .await;
                    return Err(e);
                }
            };

            let mut stream_error: Option<anyhow::Error> = None;
            'stream: loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        stop_reason = "user_aborted".to_string();
                        fatal_error = Some("user_aborted".to_string());
                        break 'turns;
                    }
                    _ = sleep(Duration::from_millis(PROVIDER_STREAM_IDLE_TIMEOUT_MS)) => {
                        stream_error = Some(anyhow!(
                            "provider stream idle timeout after {}ms",
                            PROVIDER_STREAM_IDLE_TIMEOUT_MS
                        ));
                        break 'stream;
                    }
                    evt_opt = stream.next() => {
                        let Some(event) = evt_opt else { break 'stream };
                        match event {
                            Ok(LlmEvent::TextDelta { text }) => {
                                full_text.push_str(&text);
                                turn_text.push_str(&text);
                                narration_buffer.push_str(&text);
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
                                reset_agent_message_candidate(
                                    &pool,
                                    &user_id,
                                    &session_id,
                                    task.as_ref().map(|t| t.task_id.as_str()),
                                    &event_bus,
                                    &turn_text,
                                )
                                .await?;
                                flush_agent_narration(
                                    &pool,
                                    &user_id,
                                    &session_id,
                                    task.as_ref().map(|t| t.task_id.as_str()),
                                    &event_bus,
                                    turn,
                                    &mut narration_buffer,
                                )
                                .await?;
                                let (action_kind, action_detail, queries, sources) = match &action {
                                    Some(WebSearchAction::Search { query, queries, sources }) => {
                                        ("search", query.clone(), queries.clone(), sources.clone())
                                    }
                                    Some(WebSearchAction::OpenPage { url }) => {
                                        ("open_page", url.clone(), Vec::new(), Vec::new())
                                    }
                                    Some(WebSearchAction::FindInPage { url, pattern }) => {
                                        ("find_in_page", format!("{pattern} @ {url}"), Vec::new(), Vec::new())
                                    }
                                    Some(WebSearchAction::Other) => ("other", String::new(), Vec::new(), Vec::new()),
                                    None => ("unknown", String::new(), Vec::new(), Vec::new()),
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
                                        "queries": queries,
                                        "sources": sources,
                                    }),
                                )
                                .await?;
                            }
                            Ok(LlmEvent::FunctionCall { call_id, name, arguments }) => {
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
                                stream_error = Some(e);
                                break 'stream;
                            }
                        }
                    }
                }
            }

            let Some(e) = stream_error else {
                break;
            };

            if provider_retries < MAX_PROVIDER_RETRIES {
                provider_retries += 1;
                let delay_ms = provider_retry_delay_ms(provider_retries);
                publish_provider_retry(
                    &pool,
                    &user_id,
                    &session_id,
                    task.as_ref().map(|t| t.task_id.as_str()),
                    &event_bus,
                    provider.name(),
                    provider_retries,
                    delay_ms,
                    &e.to_string(),
                )
                .await?;
                if !wait_retry(delay_ms, &cancel).await {
                    stop_reason = "user_aborted".to_string();
                    fatal_error = Some("user_aborted".to_string());
                    break 'turns;
                }
                continue;
            }

            publish_provider_error(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                provider.name(),
                &e.to_string(),
            )
            .await?;
            if let Some(t) = task.as_ref() {
                fail_task(
                    &pool,
                    &user_id,
                    &session_id,
                    &event_bus,
                    &t.task_id,
                    &format!("provider_error: {}", e),
                )
                .await?;
            }
            persist_partial_agent_message(
                &pool,
                &user_id,
                &session_id,
                &full_text,
                task.as_ref().map(|t| t.task_id.as_str()),
            )
            .await;
            return Err(e);
        }

        // No tool calls this turn → model is done.
        if pending_calls.is_empty() {
            if let Some(guard) = plan_guard(&pool, &user_id, &session_id, task.as_ref()).await? {
                reset_agent_message_candidate(
                    &pool,
                    &user_id,
                    &session_id,
                    task.as_ref().map(|t| t.task_id.as_str()),
                    &event_bus,
                    &turn_text,
                )
                .await?;
                if plan_guard_rewrites < MAX_PLAN_GUARD_REWRITES {
                    plan_guard_rewrites += 1;
                    stop_reason = "plan_guard_continue".to_string();
                    publish_and_persist(
                        &pool,
                        &user_id,
                        &session_id,
                        task.as_ref().map(|t| t.task_id.as_str()),
                        &event_bus,
                        "agent_narration",
                        serde_json::json!({
                            "turn": turn,
                            "text": format!("当前计划还没完成：{}。我会继续执行，不把待办交还给你。", guard.reason),
                        }),
                    )
                    .await?;
                    additional_inputs.push(plan_guard_input(&guard, &turn_text));
                    continue 'turns;
                }
                stop_reason = "plan_guard_exhausted".to_string();
                let message = format!(
                    "agent stopped before completing active plan: {}",
                    guard.reason
                );
                fatal_error = Some(message.clone());
                publish_and_persist(
                    &pool,
                    &user_id,
                    &session_id,
                    task.as_ref().map(|t| t.task_id.as_str()),
                    &event_bus,
                    "error",
                    serde_json::json!({ "message": message }),
                )
                .await?;
                break 'turns;
            }
            final_text = turn_text.clone();
            break 'turns;
        }

        reset_agent_message_candidate(
            &pool,
            &user_id,
            &session_id,
            task.as_ref().map(|t| t.task_id.as_str()),
            &event_bus,
            &turn_text,
        )
        .await?;
        flush_agent_narration(
            &pool,
            &user_id,
            &session_id,
            task.as_ref().map(|t| t.task_id.as_str()),
            &event_bus,
            turn,
            &mut narration_buffer,
        )
        .await?;

        for call in &pending_calls {
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "tool_call",
                serde_json::json!({
                    "status": "in_progress",
                    "call_id": &call.call_id,
                    "name": &call.name,
                    "arguments": &call.arguments,
                }),
            )
            .await?;
        }

        // Execute pending tools sequentially (parallelism can come later;
        // most tools we'll ship are I/O bound so order rarely matters but
        // serializing keeps the audit trail simple).
        let mut user_question: Option<serde_json::Value> = None;
        for call in pending_calls {
            let tool_started = Instant::now();
            vault_tool_runs::start(
                &pool,
                &user_id,
                &call.call_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                "main_agent",
                &call.name,
                &call.arguments,
            )
            .await?;

            let exec_result = tools
                .dispatch(&call.name, &call.arguments, cancel.clone(), &ctx)
                .await;

            // Treat tool errors as a delivered output: the model sees the
            // error string and decides what to do (retry / give up / keep
            // going). We do NOT propagate as Err — that would kill the turn.
            let (output_str, status, error) = match exec_result {
                Ok(s) => (s, "completed", None),
                Err(e) => {
                    let msg = e.to_string();
                    (format!("[tool error: {msg}]"), "error", Some(msg))
                }
            };
            if call.name == tools::ask_user_question::TOOL_NAME && error.is_none() {
                user_question = parse_user_question_output(&output_str);
            }
            let duration_ms = tool_started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(i64::MAX);
            let result_json = serde_json::json!({
                "output": output_str,
                "output_bytes": output_str.len(),
            });
            vault_tool_runs::finish(
                &pool,
                &user_id,
                &call.call_id,
                Some(&result_json),
                error.is_none(),
                error.as_deref(),
                duration_ms,
            )
            .await?;

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
                    "duration_ms": duration_ms,
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

        if let Some(payload) = user_question {
            awaiting_user = true;
            stop_reason = "awaiting_user".to_string();
            let question_text = payload
                .get("question_text")
                .and_then(|v| v.as_str())
                .unwrap_or("请补充一下你的要求。")
                .to_string();
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "clarification_requested",
                serde_json::json!({
                    "question": question_text,
                    "questions": payload.get("questions").cloned().unwrap_or(serde_json::Value::Null),
                }),
            )
            .await?;
            final_text = question_text;
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                task.as_ref().map(|t| t.task_id.as_str()),
                &event_bus,
                "agent_message_delta",
                serde_json::json!({ "text": final_text.clone() }),
            )
            .await?;
            break 'turns;
        }

        turn += 1;
    }

    let has_content = fatal_error.is_none() && !final_text.trim().is_empty();

    let msg_seq = if has_content {
        Some(
            vault_messages::insert(
                &pool,
                &user_id,
                &session_id,
                "agent",
                &serde_json::json!({ "type": "text", "text": final_text }),
                task.as_ref().map(|t| t.task_id.as_str()),
            )
            .await?,
        )
    } else {
        None
    };

    if let Some(t) = task.as_ref() {
        if let Some(reason) = fatal_error.as_deref() {
            fail_task(&pool, &user_id, &session_id, &event_bus, &t.task_id, reason).await?;
        } else if awaiting_user {
            vault_tasks::mark_awaiting_user(&pool, &user_id, &t.task_id).await?;
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                Some(&t.task_id),
                &event_bus,
                "task_awaiting_user",
                serde_json::json!({ "task_id": t.task_id }),
            )
            .await?;
        } else {
            if has_content {
                let deliverable_id = vault_tasks::write_deliverable(
                    &pool,
                    &user_id,
                    &t.task_id,
                    &t.expected_deliverable,
                    &final_text,
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

async fn fail_task(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    event_bus: &EventBus,
    task_id: &str,
    reason: &str,
) -> Result<()> {
    vault_tasks::mark_failed(pool, user_id, task_id, reason).await?;
    publish_and_persist(
        pool,
        user_id,
        session_id,
        Some(task_id),
        event_bus,
        "task_failed",
        serde_json::json!({
            "task_id": task_id,
            "reason": reason,
        }),
    )
    .await
}

fn provider_retry_delay_ms(retry: usize) -> u64 {
    let exponent = retry.saturating_sub(1).min(5);
    (PROVIDER_RETRY_BASE_MS * (1_u64 << exponent)).min(PROVIDER_RETRY_MAX_MS)
}

async fn wait_retry(delay_ms: u64, cancel: &CancellationToken) -> bool {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        _ = sleep(Duration::from_millis(delay_ms)) => true,
    }
}

fn parse_user_question_output(output: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    if value.get("status").and_then(|v| v.as_str()) != Some("awaiting_user") {
        return None;
    }
    let question_text = value.get("question_text").and_then(|v| v.as_str())?;
    if question_text.trim().is_empty() {
        return None;
    }
    Some(value)
}

#[derive(Debug, Clone)]
struct PlanGuard {
    reason: String,
    items: Vec<vault_plans::PlanItemRow>,
}

async fn active_plan_reminder_input(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task: Option<&TaskBinding>,
) -> Result<Option<serde_json::Value>> {
    let task_id = task.map(|t| t.task_id.as_str());
    let items = vault_plans::list_current(pool, user_id, session_id, task_id).await?;
    if items.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::json!({
        "role": "user",
        "content": format!(
            "ACTIVE PLAN STATUS\n{}\n\n继续从当前计划状态推进。完成项目后必须调用 update_plan 更新状态；所有项目 completed 后才输出最终结论。",
            format_plan_items(&items)
        ),
    })))
}

async fn plan_guard(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task: Option<&TaskBinding>,
) -> Result<Option<PlanGuard>> {
    let task_id = task.map(|t| t.task_id.as_str());
    let items = vault_plans::list_current(pool, user_id, session_id, task_id).await?;
    if items.is_empty() {
        return Ok(task.map(|_| PlanGuard {
            reason: "还没有为这个研究任务创建 active plan".to_string(),
            items,
        }));
    }
    let incomplete_count = items
        .iter()
        .filter(|item| item.status != "completed")
        .count();
    if incomplete_count == 0 {
        return Ok(None);
    }
    Ok(Some(PlanGuard {
        reason: format!("{incomplete_count} 个计划项仍未完成"),
        items,
    }))
}

fn plan_guard_input(guard: &PlanGuard, draft: &str) -> serde_json::Value {
    serde_json::json!({
        "role": "user",
        "content": format!(
            "SYSTEM PLAN GATE\n\
             你的上一版不能作为最终回答。\n\
             原因：{}\n\n\
             当前 active plan：\n{}\n\n\
             上一版草稿摘录：\n{}\n\n\
             继续执行计划。若还没有计划，先调用 update_plan 创建计划，参数使用 plan 数组；若计划项未完成，继续调用工具完成它们；\
             完成后调用 update_plan 标记 completed 并写入 evidence。不要把未完成计划交还给用户。",
            guard.reason,
            format_plan_items(&guard.items),
            preview(draft.trim(), 1200)
        ),
    })
}

fn format_plan_items(items: &[vault_plans::PlanItemRow]) -> String {
    if items.is_empty() {
        return "(no active plan)".to_string();
    }
    items
        .iter()
        .map(|item| {
            let evidence = item
                .evidence
                .as_deref()
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!(" evidence: {}", preview(text, 240)))
                .unwrap_or_default();
            format!(
                "- [{}] {}. {}{}",
                item.status, item.item_id, item.step, evidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::too_many_arguments)]
async fn publish_provider_retry(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    provider: &str,
    retry: usize,
    delay_ms: u64,
    error: &str,
) -> Result<()> {
    publish_and_persist(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "provider_retry",
        serde_json::json!({
            "provider": provider,
            "retry": retry,
            "max_retries": MAX_PROVIDER_RETRIES,
            "delay_ms": delay_ms,
            "message": error,
        }),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn publish_provider_error(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    provider: &str,
    error: &str,
) -> Result<()> {
    publish_and_persist(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "error",
        serde_json::json!({
            "provider": provider,
            "message": error,
            "max_retries": MAX_PROVIDER_RETRIES,
        }),
    )
    .await
}

async fn persist_partial_agent_message(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    full_text: &str,
    task_id: Option<&str>,
) {
    if full_text.trim().is_empty() {
        return;
    }
    let _ = vault_messages::insert(
        pool,
        user_id,
        session_id,
        "agent",
        &serde_json::json!({ "type": "text", "text": full_text }),
        task_id,
    )
    .await;
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

async fn flush_agent_narration(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    turn: usize,
    buffer: &mut String,
) -> Result<()> {
    let text = buffer.trim().to_string();
    buffer.clear();
    if text.is_empty() {
        return Ok(());
    }
    publish_and_persist(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "agent_narration",
        serde_json::json!({
            "turn": turn,
            "text": text,
        }),
    )
    .await
}

async fn reset_agent_message_candidate(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    candidate: &str,
) -> Result<()> {
    if candidate.trim().is_empty() {
        return Ok(());
    }
    publish_and_persist(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "agent_message_reset",
        serde_json::json!({}),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn plan_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE agent_plan_items (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                task_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                step TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('pending', 'in_progress', 'completed')),
                evidence TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, task_id, item_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn task() -> TaskBinding {
        TaskBinding {
            task_id: "task-1".to_string(),
            expected_deliverable: "decision_draft".to_string(),
        }
    }

    #[tokio::test]
    async fn plan_guard_requires_plan_for_task() {
        let pool = plan_pool().await;
        let binding = task();
        let guard = plan_guard(&pool, "u", "s", Some(&binding))
            .await
            .unwrap()
            .unwrap();
        assert!(guard.reason.contains("还没有"));
    }

    #[tokio::test]
    async fn plan_guard_ignores_empty_free_chat() {
        let pool = plan_pool().await;
        assert!(plan_guard(&pool, "u", "s", None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn plan_guard_catches_incomplete_plan() {
        let pool = plan_pool().await;
        let binding = task();
        vault_plans::replace_current(
            &pool,
            "u",
            "s",
            Some(&binding.task_id),
            &[
                vault_plans::PlanItemInput {
                    id: Some("p1".to_string()),
                    step: "建立研究框架".to_string(),
                    status: "completed".to_string(),
                    evidence: Some("已调用 corpus_search".to_string()),
                },
                vault_plans::PlanItemInput {
                    id: Some("p2".to_string()),
                    step: "核验行业事实".to_string(),
                    status: "pending".to_string(),
                    evidence: None,
                },
            ],
        )
        .await
        .unwrap();

        let guard = plan_guard(&pool, "u", "s", Some(&binding))
            .await
            .unwrap()
            .unwrap();
        assert!(guard.reason.contains("1 个计划项"));
    }

    #[tokio::test]
    async fn plan_guard_catches_free_chat_active_plan() {
        let pool = plan_pool().await;
        vault_plans::replace_current(
            &pool,
            "u",
            "s",
            None,
            &[vault_plans::PlanItemInput {
                id: Some("p1".to_string()),
                step: "核验事实".to_string(),
                status: "in_progress".to_string(),
                evidence: None,
            }],
        )
        .await
        .unwrap();

        let guard = plan_guard(&pool, "u", "s", None).await.unwrap().unwrap();
        assert!(guard.reason.contains("1 个计划项"));
    }

    #[tokio::test]
    async fn plan_guard_allows_completed_plan() {
        let pool = plan_pool().await;
        let binding = task();
        vault_plans::replace_current(
            &pool,
            "u",
            "s",
            Some(&binding.task_id),
            &[vault_plans::PlanItemInput {
                id: Some("p1".to_string()),
                step: "完成综合判断".to_string(),
                status: "completed".to_string(),
                evidence: Some("事实、反方和风险已综合".to_string()),
            }],
        )
        .await
        .unwrap();

        assert!(plan_guard(&pool, "u", "s", Some(&binding))
            .await
            .unwrap()
            .is_none());
    }
}
