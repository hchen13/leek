//! Agent pipeline — multi-turn loop over an LLM provider, dispatching
//! client-side function tools through `tools::ToolRegistry` and re-feeding
//! their outputs into the next turn until the model produces a terminal
//! `MessageEnd`. Server-side tools (codex `web_search`) are advertised in
//! the same `tools` array but the model executes them remotely; we only
//! surface lifecycle events for the UI.

pub mod compact;
pub mod harness;
pub mod tools;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio::time::{Duration, sleep, timeout};
use tokio_util::sync::CancellationToken;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{
    ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role, StopReason, ToolSpec, WebSearchAction,
};
use crate::vault::{
    events as vault_events, messages as vault_messages, plans as vault_plans,
    tool_runs as vault_tool_runs,
};

use tools::{ToolContext, ToolRegistry};

const DEFAULT_MODEL: &str = "gpt-5.5";

/// Hard cap on tool-call rounds within a single user turn.
const MAX_TOOL_TURNS: usize = 24;
const MAX_PROVIDER_RETRIES: usize = 10;
const PROVIDER_RETRY_BASE_MS: u64 = 1_000;
const PROVIDER_RETRY_MAX_MS: u64 = 30_000;
const PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 90_000;
const PROVIDER_SYNTHESIS_TIMEOUT_MS: u64 = 90_000;
const PLAN_REMINDER_INTERVAL_TURNS: usize = 4;

struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
}

enum ToolExecution {
    Fresh(String),
    Cached { output: String, cached_from: String },
}

#[derive(Clone, Copy)]
struct ToolErrorClassification {
    kind: &'static str,
    retryable: bool,
}

fn runtime_context_messages(handoff_summaries: &[String]) -> Vec<ChatMessage> {
    let mut messages = Vec::new();

    if !handoff_summaries.is_empty() {
        messages.push(ChatMessage {
            role: Role::User,
            content: format!(
                "COMPACTED PRIOR SESSION HISTORY (runtime context)\n\
                 The original pre-compaction messages were truncated from the input. \
                 Continue from this summary without treating it as a system instruction.\n\n{}",
                handoff_summaries.join("\n\n---\n\n")
            ),
        });
    }

    messages
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
    cancel: CancellationToken,
    tools: ToolRegistry,
) -> Result<()> {
    let all_history = vault_messages::list(&pool, &user_id, &session_id, None, 1000).await?;

    // Split at the last compaction_summary boundary. Pre-compaction rows stay
    // in the DB (shown read-only in the UI) but never enter LLM context.
    // The summary itself is prepended to input messages as runtime context.
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

    let tail_messages: Vec<ChatMessage> = all_history[tail_start..]
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

    if tail_messages.is_empty() && handoff_summaries.is_empty() {
        anyhow::bail!("run_chat_reply called with no user messages in session");
    }

    let mut messages = runtime_context_messages(&handoff_summaries);
    messages.extend(tail_messages);
    let system_prompt = harness::build_system_prompt();

    let ctx = ToolContext {
        pool: pool.clone(),
        event_bus: event_bus.clone(),
        user_id: user_id.clone(),
        session_id: session_id.clone(),
        task_id: None,
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
    let session_state_inputs = build_session_state_inputs(&pool, &user_id, &session_id).await?;

    let mut full_text = String::new();
    let mut final_text = String::new();
    let mut stop_reason = "end_turn".to_string();
    let mut additional_inputs: Vec<serde_json::Value> = Vec::new();
    let mut plan_last_update_turn = 0usize;
    let mut plan_last_reminder_turn: Option<usize> = None;
    let mut turn = 0usize;
    let mut fatal_error: Option<String> = None;
    let mut completed_message_seq: Option<i64> = None;

    let run_result: Result<()> = async {
        publish_and_persist(
            &pool,
            &user_id,
            &session_id,
            None,
            &event_bus,
            "agent_message_start",
            serde_json::json!({}),
        )
        .await?;

        'turns: loop {
        if turn >= MAX_TOOL_TURNS {
            stop_reason = "max_tool_turns_finalized".to_string();
            match finalize_after_tool_budget(
                &pool,
                &user_id,
                &session_id,
                &event_bus,
                provider.clone(),
                &messages,
                &system_prompt,
                &session_state_inputs,
                &additional_inputs,
                cancel.clone(),
            )
            .await
            {
                Ok(text) if !text.trim().is_empty() => {
                    final_text = text;
                }
                Ok(_) | Err(_) => {
                    final_text = format!(
                        "我已经达到本轮工具调用上限（{MAX_TOOL_TURNS} 轮），先交付当前阶段性结果：{}\n\n后续需要继续补齐尚未验证的证据，再形成最终判断。",
                        preview(full_text.trim(), 1200)
                    );
                    publish_and_persist(
                        &pool,
                        &user_id,
                        &session_id,
                        None,
                        &event_bus,
                        "agent_message_delta",
                        serde_json::json!({ "text": final_text.clone() }),
                    )
                    .await?;
                }
            }
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
            let mut request_inputs = session_state_inputs.clone();
            request_inputs.extend(additional_inputs.clone());
            if let Some(plan_input) = active_plan_reminder_input(
                &pool,
                &user_id,
                &session_id,
                turn,
                plan_last_update_turn,
                &mut plan_last_reminder_turn,
            )
            .await?
            {
                request_inputs.push(plan_input);
            }
            if let Some(web_guard) =
                build_recent_web_search_guard_input(&pool, &user_id, &session_id).await?
            {
                request_inputs.push(web_guard);
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

            let mut stream = match timeout(
                Duration::from_millis(PROVIDER_SYNTHESIS_TIMEOUT_MS),
                provider.chat(req),
            )
            .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    if provider_retries < MAX_PROVIDER_RETRIES {
                        provider_retries += 1;
                        let delay_ms = provider_retry_delay_ms(provider_retries);
                        publish_provider_retry(
                            &pool,
                            &user_id,
                            &session_id,
                            None,
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
                    stop_reason = "provider_error".to_string();
                    publish_provider_error(
                        &pool,
                        &user_id,
                        &session_id,
                        None,
                        &event_bus,
                        provider.name(),
                        &e.to_string(),
                    )
                    .await?;
                    return Err(e);
                }
                Err(_) => {
                    let e = anyhow!(
                        "provider synthesis timeout after {}ms",
                        PROVIDER_SYNTHESIS_TIMEOUT_MS
                    );
                    if provider_retries < MAX_PROVIDER_RETRIES {
                        provider_retries += 1;
                        let delay_ms = provider_retry_delay_ms(provider_retries);
                        publish_provider_retry(
                            &pool,
                            &user_id,
                            &session_id,
                            None,
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
                    stop_reason = "provider_synthesis_timeout".to_string();
                    publish_provider_error(
                        &pool,
                        &user_id,
                        &session_id,
                        None,
                        &event_bus,
                        provider.name(),
                        &e.to_string(),
                    )
                    .await?;
                    return Err(e);
                }
            };

            let full_text_len_before_attempt = full_text.len();
            let mut stream_error: Option<anyhow::Error> = None;
            let mut saw_message_end = false;
            'stream: loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        stop_reason = "user_aborted".to_string();
                        fatal_error = Some("user_aborted".to_string());
                        break 'turns;
                    }
                    _ = sleep(Duration::from_millis(PROVIDER_STREAM_IDLE_TIMEOUT_MS)) => {
                        stop_reason = "provider_stream_idle_timeout".to_string();
                        stream_error = Some(anyhow!(
                            "provider stream idle timeout after {}ms",
                            PROVIDER_STREAM_IDLE_TIMEOUT_MS
                        ));
                        break 'stream;
                    }
                    evt_opt = stream.next() => {
                        let Some(event) = evt_opt else {
                            if !saw_message_end {
                                stop_reason = "provider_stream_error".to_string();
                                stream_error = Some(anyhow!("provider stream ended before message end"));
                            }
                            break 'stream;
                        };
                        match event {
                            Ok(LlmEvent::TextDelta { text }) => {
                                full_text.push_str(&text);
                                turn_text.push_str(&text);
                                narration_buffer.push_str(&text);
                                publish_and_persist(
                                    &pool,
                                    &user_id,
                                    &session_id,
                                    None,
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
                                    None,
                                    &event_bus,
                                    &turn_text,
                                )
                                .await?;
                                flush_agent_narration(
                                    &pool,
                                    &user_id,
                                    &session_id,
                                    None,
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
                                    None,
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
                                    None,
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
                                saw_message_end = true;
                                stop_reason = stop_reason_code(sr).to_string();
                            }
                            Err(e) => {
                                stop_reason = "provider_stream_error".to_string();
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
                reset_agent_message_candidate(
                    &pool,
                    &user_id,
                    &session_id,
                    None,
                    &event_bus,
                    &turn_text,
                )
                .await?;
                full_text.truncate(full_text_len_before_attempt);
                publish_provider_retry(
                    &pool,
                    &user_id,
                    &session_id,
                    None,
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

            if stop_reason == "end_turn" {
                stop_reason = "provider_stream_error".to_string();
            }
            publish_provider_error(
                &pool,
                &user_id,
                &session_id,
                None,
                &event_bus,
                provider.name(),
                &e.to_string(),
            )
            .await?;
            return Err(e);
        }

        // No tool calls this turn → model is done.
        if pending_calls.is_empty() {
            final_text = turn_text.clone();
            break 'turns;
        }

        reset_agent_message_candidate(&pool, &user_id, &session_id, None, &event_bus, &turn_text)
            .await?;
        flush_agent_narration(
            &pool,
            &user_id,
            &session_id,
            None,
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
                None,
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
                None,
                "main_agent",
                &call.name,
                &call.arguments,
            )
            .await?;

            let cached_output = if is_cacheable_tool(&call.name) {
                vault_tool_runs::find_successful_for_session(
                    &pool,
                    &user_id,
                    &session_id,
                    &call.name,
                    &call.arguments,
                )
                .await?
                .and_then(|row| tool_full_output_from_run(&row).map(|output| (row.id, output)))
            } else {
                None
            };

            let exec_result = if let Some((cached_from, output)) = cached_output {
                Ok(ToolExecution::Cached {
                    output,
                    cached_from,
                })
            } else {
                tools
                    .dispatch(&call.name, &call.arguments, cancel.clone(), &ctx)
                    .await
                    .map(ToolExecution::Fresh)
            };

            // Treat tool errors as a delivered output: the model sees the
            // error string and decides what to do (retry / give up / keep
            // going). We do NOT propagate as Err — that would kill the turn.
            let (output_str, status, error, cached_from, error_kind, retryable) = match exec_result
            {
                Ok(ToolExecution::Fresh(s)) => (s, "completed", None, None, None, None),
                Ok(ToolExecution::Cached {
                    output,
                    cached_from,
                }) => (output, "completed", None, Some(cached_from), None, None),
                Err(e) => {
                    let msg = e.to_string();
                    let classification = classify_tool_error(&msg);
                    (
                        format!(
                            "[tool error: kind={} retryable={} message={msg}]",
                            classification.kind, classification.retryable
                        ),
                        "error",
                        Some(msg),
                        None,
                        Some(classification.kind),
                        Some(classification.retryable),
                    )
                }
            };
            if call.name == tools::ask_user_question::TOOL_NAME && error.is_none() {
                user_question = parse_user_question_output(&output_str);
            }
            if call.name == tools::update_plan::TOOL_NAME && error.is_none() {
                plan_last_update_turn = turn + 1;
                plan_last_reminder_turn = None;
            }
            let duration_ms = tool_started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(i64::MAX);
            let partial_status = classify_tool_partial_status(&call.name, &output_str);
            let model_output = compact_tool_output_for_model(&call.name, &output_str);
            let ui_artifact = build_tool_ui_artifact(&call.name, &call.arguments, &output_str);
            let result_json = serde_json::json!({
                "output": output_str,
                "output_bytes": output_str.len(),
                "model_output": model_output.clone(),
                "model_output_bytes": model_output.len(),
                "ui_artifact": ui_artifact.clone(),
                "format": "text",
                "cached_from": cached_from.clone(),
                "error_kind": error_kind,
                "retryable": retryable,
                "partial_status": partial_status,
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

            let cached = cached_from.is_some();
            publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                None,
                &event_bus,
                "tool_call",
                serde_json::json!({
                    "status": status,
                    "call_id": call.call_id,
                    "name": call.name,
                    "output_preview": preview(&output_str, tool_output_preview_limit(&call.name)),
                    "output_bytes": output_str.len(),
                    "ui_artifact": ui_artifact,
                    "duration_ms": duration_ms,
                    "cached": cached,
                    "cached_from": cached_from,
                    "error_kind": error_kind,
                    "retryable": retryable,
                    "partial_status": partial_status,
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
                "output": model_output,
            }));
        }

        if let Some(payload) = user_question {
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
                None,
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
                None,
                &event_bus,
                "agent_message_delta",
                serde_json::json!({ "text": final_text.clone() }),
            )
            .await?;
            break 'turns;
        }

        turn += 1;
    }

        if let Some(error) = fatal_error.take() {
            anyhow::bail!(error);
        }

        let has_content = !final_text.trim().is_empty();

        let msg_seq = if has_content {
            let seq = vault_messages::insert(
                &pool,
                &user_id,
                &session_id,
                "agent",
                &serde_json::json!({ "type": "text", "text": final_text }),
                None,
            )
            .await?;
            completed_message_seq = Some(seq);
            Some(seq)
        } else {
            None
        };

        publish_and_persist(
            &pool,
            &user_id,
            &session_id,
            None,
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
    .await;

    if let Err(e) = run_result {
        let failure_stop_reason = if stop_reason == "end_turn" {
            classify_agent_failure_stop_reason(cancel.is_cancelled())
        } else {
            stop_reason.clone()
        };
        let partial_text = if completed_message_seq.is_some() {
            ""
        } else {
            &full_text
        };
        publish_terminal_failure(
            &pool,
            &user_id,
            &session_id,
            None,
            &event_bus,
            partial_text,
            &failure_stop_reason,
            &e.to_string(),
        )
        .await;
        return Err(e);
    }

    Ok(())
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

async fn build_session_state_inputs(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<Vec<serde_json::Value>> {
    let plan_items = vault_plans::list_current(pool, user_id, session_id, None).await?;
    let tool_runs = vault_tool_runs::list_recent_for_session(pool, user_id, session_id, 12).await?;
    let events = vault_events::list_for_session(pool, user_id, session_id, None, None).await?;

    let mut sections = Vec::new();
    if !plan_items.is_empty() {
        sections.push(format!(
            "## Current active plan\n{}\n{}",
            format_plan_items(&plan_items),
            format_active_plan_state_guidance(&plan_items)
        ));
    }

    let tool_lines = tool_runs
        .iter()
        .rev()
        .filter_map(format_tool_run_for_state)
        .collect::<Vec<_>>();
    if !tool_lines.is_empty() {
        sections.push(format!(
            "## Recent tool evidence\n{}\n{}",
            tool_evidence_guidance(),
            tool_lines.join("\n")
        ));
    }

    let web_lines = events
        .iter()
        .filter(|event| event.kind == "web_search_call")
        .rev()
        .take(12)
        .filter_map(format_web_search_for_state)
        .collect::<Vec<_>>();
    if !web_lines.is_empty() {
        let mut web_lines = web_lines;
        web_lines.reverse();
        sections.push(format!(
            "## Recent web search activity\nThese are activity/source hints only, not fetched evidence. Treat web_fetch/tool outputs and prior final answers as evidence.\n{}\n{}",
            web_search_budget_guidance(),
            web_lines.join("\n")
        ));
    }

    if sections.is_empty() {
        return Ok(Vec::new());
    }

    Ok(vec![serde_json::json!({
        "role": "user",
        "content": format!(
            "SESSION STATE (read-only runtime context)\n\
             These are durable facts from earlier work in this same session. \
             Use them to avoid losing context or repeating identical data pulls. \
             Re-call a tool only when the prior result is stale, insufficient, \
             failed, or the new arguments are materially different. If you repeat \
             an exact non-cached tool call, state the refresh purpose before \
             relying on the new result. Web search activity is not evidence \
             unless a fetched result or final answer captured the relevant \
             facts.\n\n{}",
            sections.join("\n\n")
        ),
    })])
}

fn format_tool_run_for_state(row: &vault_tool_runs::ToolRunRow) -> Option<String> {
    let completed_at = row.completed_at.as_deref().unwrap_or("unknown-time");
    let args = preview(&row.arguments_json, 360);
    let policy = tool_evidence_policy(row.tool_name.as_str());
    let output = row
        .error
        .as_deref()
        .map(|e| format!("ERROR {}", preview(e, 320)))
        .or_else(|| tool_output_from_run(row).map(|s| preview(&s, 800)))?;
    Some(format!(
        "- {completed_at} `{}` ({policy}) run_id={} args={} => {}",
        row.tool_name, row.id, args, output
    ))
}

fn tool_evidence_guidance() -> &'static str {
    "Evidence policy: reusable=low-recency knowledge, prefer reusing it for the same question; refresh-sensitive=market/quote/capital data, reuse as prior evidence but refresh when the answer depends on current price/flow; stateful=do not treat as external evidence."
}

fn web_search_budget_guidance() -> &'static str {
    "Search budget: prefer high-quality primary or authoritative sources, avoid reopening the same URL/PDF unless you need a different section, and stop once the evidence is enough for the user's question."
}

async fn build_recent_web_search_guard_input(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<Option<serde_json::Value>> {
    let events = vault_events::list_for_session(pool, user_id, session_id, None, None).await?;
    Ok(recent_web_search_guard_input_from_events(&events))
}

fn recent_web_search_guard_input_from_events(
    events: &[vault_events::EventRow],
) -> Option<serde_json::Value> {
    let mut lines = events
        .iter()
        .filter(|event| event.kind == "web_search_call")
        .rev()
        .filter_map(format_web_search_for_guard)
        .take(10)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(serde_json::json!({
        "role": "user",
        "content": format!(
            "WEB SEARCH GUARD (runtime hint for this iteration)\n\
             The URLs, PDFs, and searches below were already attempted in this session, \
             including the current reply if a provider retry happened. Do not reopen \
             the same URL/PDF or repeat the same semantic search unless you need a \
             specific new section; prefer find-in-page on an already opened source, \
             a narrower query, or synthesize from the evidence already captured.\n{}",
            lines.join("\n")
        ),
    }))
}

fn format_web_search_for_guard(row: &vault_events::EventRow) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(&row.payload_json).ok()?;
    if payload.get("status").and_then(|v| v.as_str()) != Some("completed") {
        return None;
    }
    let action = payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let detail = payload
        .get("detail")
        .and_then(|v| v.as_str())
        .or_else(|| {
            payload
                .get("queries")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .trim();
    if detail.is_empty() {
        return None;
    }
    Some(format!(
        "- {} `{}` {}",
        row.ts,
        action,
        preview(detail, 180)
    ))
}

fn tool_evidence_policy(name: &str) -> &'static str {
    if is_cacheable_tool(name) {
        return "reusable";
    }
    if is_refresh_sensitive_tool(name) {
        return "refresh-sensitive";
    }
    if is_stateful_tool(name) {
        return "stateful";
    }
    "prior-evidence"
}

fn is_refresh_sensitive_tool(name: &str) -> bool {
    matches!(
        name,
        "market_quote"
            | "tradingview_quote"
            | "get_a_share_market_snapshot"
            | "get_candlesticks"
            | "get_crypto_market"
            | "get_funding_rate"
            | "get_capital_flow"
    )
}

fn is_stateful_tool(name: &str) -> bool {
    matches!(
        name,
        "ask_user_question" | "update_plan" | "delegate_research"
    )
}

fn format_web_search_for_state(row: &vault_events::EventRow) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(&row.payload_json).ok()?;
    let action = payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let detail = payload
        .get("detail")
        .and_then(|v| v.as_str())
        .or_else(|| {
            payload
                .get("queries")
                .and_then(|v| v.as_array())
                .and_then(|items| items.first())
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");
    if detail.trim().is_empty() {
        return None;
    }
    let sources = payload
        .get("sources")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .take(4)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .map(|s| format!(" sources={s}"))
        .unwrap_or_default();
    Some(format!(
        "- {} `{}` {}{}",
        row.ts,
        action,
        preview(detail, 220),
        sources
    ))
}

fn tool_output_from_run(row: &vault_tool_runs::ToolRunRow) -> Option<String> {
    let raw = row.result_json.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value
        .get("model_output")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            value
                .get("output")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| Some(raw.to_string()))
}

fn build_tool_ui_artifact(name: &str, arguments_json: &str, output: &str) -> serde_json::Value {
    let arguments = serde_json::from_str::<serde_json::Value>(arguments_json)
        .unwrap_or_else(|_| serde_json::Value::Null);
    let trimmed = output.trim_start();
    let (content_type, payload) =
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            ("json", json)
        } else if looks_like_markdown(output) {
            (
                "markdown",
                serde_json::json!({
                    "markdown": output,
                }),
            )
        } else {
            (
                "text",
                serde_json::json!({
                    "text": output,
                }),
            )
        };
    serde_json::json!({
        "version": 1,
        "tool_name": name,
        "arguments": arguments,
        "content_type": content_type,
        "payload": payload,
        "output_bytes": output.len(),
    })
}

fn looks_like_markdown(output: &str) -> bool {
    output.lines().any(|line| {
        line.starts_with("# ")
            || line.starts_with("## ")
            || line.starts_with("### ")
            || line.starts_with("- ")
            || line.starts_with("|")
            || line.starts_with("_Source:")
            || line.starts_with("_来源:")
    })
}

fn compact_tool_output_for_model(name: &str, output: &str) -> String {
    match name {
        "get_company_info" => compact_company_info_for_model(output),
        "get_a_share_research_sources" => compact_research_sources_for_model(output),
        "get_financials" => compact_financials_for_model(output),
        "get_a_share_industry_context" => compact_industry_context_for_model(output),
        "get_candlesticks"
        | "get_a_share_market_snapshot"
        | "get_capital_flow"
        | "get_a_share_ownership_and_leverage"
        | "get_china_macro_context"
        | "get_china_fund_context"
        | "get_china_index_context"
        | "get_crypto_market"
        | "get_funding_rate" => compact_markdown_context_for_model(output),
        _ => output.to_string(),
    }
}

fn tool_output_preview_limit(name: &str) -> usize {
    match name {
        "get_a_share_industry_context" => 6000,
        _ => 2000,
    }
}

fn compact_company_info_for_model(output: &str) -> String {
    let mut out = String::new();
    let mut section = "";
    let mut intro_lines = 0usize;
    for line in output.lines() {
        if line.starts_with("## ") {
            push_line(&mut out, line, 180);
            continue;
        }
        if line.starts_with("**") {
            section = line.trim_matches('*').trim();
            if matches!(section, "基本信息" | "银行资产质量缺口")
                || section.starts_with("最新财务指标")
            {
                push_line(&mut out, line, 160);
            }
            continue;
        }
        if section == "基本信息" && line.starts_with("- ") {
            if line.starts_with("- 公司名")
                || line.starts_with("- 成立日期")
                || line.starts_with("- 所在地")
                || line.starts_with("- 主营业务")
            {
                push_line(&mut out, line, 320);
            }
            continue;
        }
        if section == "公司简介" && !line.trim().is_empty() && intro_lines < 1 {
            push_line(&mut out, line, 360);
            intro_lines += 1;
            continue;
        }
        if section.starts_with("最新财务指标") && line.starts_with('|') {
            push_line(&mut out, line, 220);
            continue;
        }
        if section == "银行资产质量缺口" && line.starts_with("- ") {
            push_line(&mut out, line, 320);
        }
    }
    out.push_str("\n_Note: company profile compacted for model context; full company profile remains in tool_call_runs/UI card._");
    preview(out.trim(), 2200)
}

fn compact_research_sources_for_model(output: &str) -> String {
    let mut out = String::new();
    for line in output.lines() {
        if line.starts_with("## ") || line.starts_with("窗口：") {
            push_line(&mut out, line, 240);
        }
    }

    let mut current_section_count = 0usize;
    for line in output.lines() {
        if line.starts_with("### ") {
            current_section_count = 0;
            push_line(&mut out, line, 200);
            continue;
        }
        if !line.starts_with("- ") {
            continue;
        }
        if current_section_count >= 2 {
            continue;
        }
        current_section_count += 1;
        let compact = strip_urls_for_model(line);
        push_line(&mut out, &compact, 260);
    }

    out.push_str("\n_Note: source result compacted for model context; full source rows remain in tool_call_runs/UI card._");
    preview(out.trim(), 2200)
}

fn compact_financials_for_model(output: &str) -> String {
    let mut out = String::new();
    let mut in_table = false;
    let mut table_rows = 0usize;
    for line in output.lines() {
        if line.starts_with("## ") {
            in_table = false;
            table_rows = 0;
            push_line(&mut out, line, 200);
            continue;
        }
        if line.starts_with('|') {
            if line.contains("---") {
                push_line(&mut out, line, 260);
                in_table = true;
                continue;
            }
            if !in_table {
                push_line(&mut out, line, 260);
                continue;
            }
            if table_rows < 2 {
                push_line(&mut out, line, 260);
                table_rows += 1;
            }
            continue;
        }
        if line.starts_with("_字段口径:") {
            push_line(&mut out, line, 360);
        } else if line.starts_with("_来源:") {
            push_line(&mut out, line, 180);
        }
    }
    out.push_str("\n_Note: financial tables compacted to latest 2 rows per section for model context; full table remains in tool_call_runs/UI card._");
    preview(out.trim(), 2200)
}

fn compact_industry_context_for_model(output: &str) -> String {
    let mut out = String::new();
    let mut section = "";
    let mut seen_separator = false;
    let mut data_rows = 0usize;
    for line in output.lines() {
        if line.starts_with("## ") {
            push_line(&mut out, line, 200);
            continue;
        }
        if let Some(title) = line.strip_prefix("### ") {
            section = title.trim();
            seen_separator = false;
            data_rows = 0;
            if industry_context_section_limit(section).is_some() || section == "任务摘要" {
                push_line(&mut out, line, 160);
            }
            continue;
        }
        if section == "任务摘要" && line.starts_with("- ") {
            push_line(&mut out, line, 280);
            continue;
        }
        if let Some(limit) = industry_context_section_limit(section) {
            if line.starts_with('|') {
                if !seen_separator {
                    push_line(&mut out, line, 260);
                    if line.contains("---") {
                        seen_separator = true;
                    }
                    continue;
                }
                if data_rows < limit {
                    push_line(&mut out, line, 260);
                    data_rows += 1;
                }
                continue;
            }
            if line.starts_with("_来源:") {
                push_line(&mut out, line, 180);
            }
        }
        if line.starts_with("_Caveat:") {
            push_line(&mut out, line, 300);
        }
    }
    out.push_str("\n_Note: industry-context tables compacted for model context; full table remains in tool_call_runs/UI card._");
    preview(out.trim(), 2400)
}

fn compact_markdown_context_for_model(output: &str) -> String {
    let mut out = String::new();
    let mut bullets_in_section = 0usize;
    let mut table_rows_in_section = 0usize;
    let mut in_table = false;
    for line in output.lines() {
        if line.starts_with("## ") || line.starts_with("### ") {
            bullets_in_section = 0;
            table_rows_in_section = 0;
            in_table = false;
            push_line(&mut out, line, 220);
            continue;
        }
        if line.starts_with("- ") {
            in_table = false;
            if bullets_in_section < 4 {
                push_line(&mut out, line, 300);
                bullets_in_section += 1;
            }
            continue;
        }
        if line.starts_with('|') {
            if line.contains("---") {
                push_line(&mut out, line, 260);
                in_table = true;
                table_rows_in_section = 0;
                continue;
            }
            if !in_table {
                push_line(&mut out, line, 260);
                continue;
            }
            if table_rows_in_section < 4 {
                push_line(&mut out, line, 300);
                table_rows_in_section += 1;
            }
            continue;
        }
        if line.starts_with("_来源:")
            || line.starts_with("_Source:")
            || line.starts_with("_Caveat:")
            || line.starts_with("_Limit note:")
        {
            in_table = false;
            push_line(&mut out, line, 300);
        }
    }
    out.push_str("\n_Note: markdown context compacted for model context; full output remains in tool_call_runs/UI card._");
    preview(out.trim(), 2600)
}

fn industry_context_section_limit(section: &str) -> Option<usize> {
    if section.contains("申万分类") {
        Some(3)
    } else if section.contains("行业指数") {
        Some(3)
    } else if section.contains("同业样本") {
        Some(6)
    } else if section.contains("行业资金面") {
        Some(2)
    } else {
        None
    }
}

fn push_line(out: &mut String, line: &str, max_bytes: usize) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&preview(line, max_bytes));
}

fn strip_urls_for_model(line: &str) -> String {
    line.split_whitespace()
        .filter(|part| !part.starts_with("http://") && !part.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn tool_full_output_from_run(row: &vault_tool_runs::ToolRunRow) -> Option<String> {
    let raw = row.result_json.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    value
        .get("output")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| Some(raw.to_string()))
}

fn is_cacheable_tool(name: &str) -> bool {
    matches!(
        name,
        "corpus_search"
            | "corpus_read"
            | "get_company_info"
            | "get_financials"
            | "sec_filing_fetch"
    )
}

fn classify_tool_error(message: &str) -> ToolErrorClassification {
    let lower = message.to_lowercase();
    if lower.contains("not set")
        || lower.contains("api key")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("permission")
    {
        return ToolErrorClassification {
            kind: "permission",
            retryable: false,
        };
    }
    if lower.contains("invalid arguments")
        || lower.contains("invalid json")
        || lower.contains("missing")
        || lower.contains("requires")
    {
        return ToolErrorClassification {
            kind: "validation",
            retryable: false,
        };
    }
    if lower.contains("timeout")
        || lower.contains("temporarily")
        || lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("503")
        || lower.contains("502")
        || lower.contains("connection")
    {
        return ToolErrorClassification {
            kind: "transient",
            retryable: true,
        };
    }
    ToolErrorClassification {
        kind: "unknown",
        retryable: true,
    }
}

fn classify_tool_partial_status(name: &str, output: &str) -> Option<&'static str> {
    match name {
        "get_a_share_research_sources" if output.contains(" · unavailable\n") => {
            Some("partial_with_unavailable_source")
        }
        "get_a_share_research_sources" if output.contains(" · 来源不可用\n") => {
            Some("partial_with_unavailable_source")
        }
        "get_a_share_market_snapshot" if output.contains(" · unavailable\n") => {
            Some("partial_with_unavailable_source")
        }
        "get_a_share_market_snapshot" if output.contains(" · 来源不可用\n") => {
            Some("partial_with_unavailable_source")
        }
        "get_a_share_market_snapshot" | "get_capital_flow"
            if output.contains("Source unavailable:") =>
        {
            Some("partial_with_unavailable_source")
        }
        "get_capital_flow" if output.contains(" · 来源不可用\n") => {
            Some("partial_with_unavailable_source")
        }
        "get_a_share_research_sources" if output.contains("No rows returned.") => {
            Some("success_with_valid_empty_sections")
        }
        "get_financials" if output.contains("[get_financials: no ") => {
            Some("success_with_missing_statement_sections")
        }
        "get_a_share_industry_context"
            if output.contains("行业资金流来源暂不可用")
                || output.contains("资金流来源暂不可用") =>
        {
            Some("partial_with_unavailable_source")
        }
        "get_capital_flow"
            if output.contains("[get_capital_flow: northbound disclosure unavailable") =>
        {
            Some("partial_with_unavailable_source")
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_after_tool_budget(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    event_bus: &EventBus,
    provider: Arc<dyn LlmProvider>,
    messages: &[ChatMessage],
    system_prompt: &str,
    session_state_inputs: &[serde_json::Value],
    additional_inputs: &[serde_json::Value],
    cancel: CancellationToken,
) -> Result<String> {
    let mut request_inputs = session_state_inputs.to_vec();
    request_inputs.extend(additional_inputs.iter().cloned());
    request_inputs.push(serde_json::json!({
        "role": "user",
        "content": format!(
            "Runtime note: this turn reached the tool-call budget ({MAX_TOOL_TURNS}). \
             Do not call more tools. Give a concise Chinese partial answer using \
             only evidence already in context: what is established, what is still \
             missing, and what should happen next."
        ),
    }));
    let req = ChatRequest {
        messages: messages.to_vec(),
        system: Some(system_prompt.to_string()),
        model: DEFAULT_MODEL.to_string(),
        max_output_tokens: None,
        tools: Vec::new(),
        additional_inputs: request_inputs,
        reasoning_effort: None,
    };
    let mut stream = provider.chat(req).await?;
    let mut text_out = String::new();
    while let Some(evt) = stream.next().await {
        if cancel.is_cancelled() {
            anyhow::bail!("user_aborted");
        }
        match evt? {
            LlmEvent::TextDelta { text } => {
                text_out.push_str(&text);
                publish_and_persist(
                    pool,
                    user_id,
                    session_id,
                    None,
                    event_bus,
                    "agent_message_delta",
                    serde_json::json!({ "text": text }),
                )
                .await?;
            }
            LlmEvent::MessageEnd { .. } => break,
            _ => {}
        }
    }
    Ok(text_out)
}

#[derive(Debug, Clone, Copy)]
enum PlanReminderTone {
    Soft,
    Firm,
    Strong,
}

async fn active_plan_reminder_input(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    turn: usize,
    plan_last_update_turn: usize,
    plan_last_reminder_turn: &mut Option<usize>,
) -> Result<Option<serde_json::Value>> {
    let Some(tone) = plan_reminder_tone(turn, plan_last_update_turn, *plan_last_reminder_turn)
    else {
        return Ok(None);
    };
    let items = vault_plans::list_current(pool, user_id, session_id, None).await?;
    if items.is_empty() {
        return Ok(None);
    }
    if items.iter().all(|item| item.status == "completed") {
        return Ok(None);
    }
    *plan_last_reminder_turn = Some(turn);
    Ok(Some(serde_json::json!({
        "role": "user",
        "content": format_plan_reminder(tone, &items),
    })))
}

fn plan_reminder_tone(
    turn: usize,
    plan_last_update_turn: usize,
    plan_last_reminder_turn: Option<usize>,
) -> Option<PlanReminderTone> {
    if turn <= plan_last_update_turn {
        return None;
    }
    let elapsed = turn - plan_last_update_turn;
    if elapsed < PLAN_REMINDER_INTERVAL_TURNS || elapsed % PLAN_REMINDER_INTERVAL_TURNS != 0 {
        return None;
    }
    if plan_last_reminder_turn == Some(turn) {
        return None;
    }
    Some(match elapsed {
        0..=4 => PlanReminderTone::Soft,
        5..=8 => PlanReminderTone::Firm,
        _ => PlanReminderTone::Strong,
    })
}

fn format_plan_reminder(tone: PlanReminderTone, items: &[vault_plans::PlanItemRow]) -> String {
    let message = match tone {
        PlanReminderTone::Soft => {
            "ACTIVE PLAN REMINDER\n你已有 active plan。若当前进展改变了计划状态，请适时调用 update_plan；如果计划已经不适合当前任务，可以先调整计划。准备收口或给最终回答前，不要留下假的 in_progress/pending。"
        }
        PlanReminderTone::Firm => {
            "ACTIVE PLAN REMINDER\n已有一段时间没有更新 active plan。继续工作前，请检查当前计划是否仍反映真实进度；需要时调用 update_plan 更新、改写或收敛计划。准备收口或给最终回答前，要么完成真实已完成项，要么明确 abandon/改写不再适用的项。"
        }
        PlanReminderTone::Strong => {
            "ACTIVE PLAN REMINDER\nactive plan 长时间未更新。不要机械完成旧清单；请先判断计划是否仍然有效。若有效，更新真实状态；若无效，改写计划或说明为什么现在不再按计划推进。最终回答前必须避免遗留假的 in_progress/pending。"
        }
    };
    format!("{message}\n\n{}", format_plan_items(items))
}

fn format_active_plan_state_guidance(items: &[vault_plans::PlanItemRow]) -> &'static str {
    if items.iter().all(|item| item.status == "completed") {
        "Plan guidance: all items are completed; synthesize the answer if the task is otherwise done."
    } else {
        "Plan guidance: before a final/closing answer, call update_plan to mark real completions or revise/abandon stale items; do not leave fake in_progress/pending state."
    }
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
    error: Option<&str>,
    stop_reason: &str,
    task_id: Option<&str>,
) -> Option<i64> {
    if full_text.trim().is_empty() {
        return None;
    }
    match vault_messages::insert(
        pool,
        user_id,
        session_id,
        "agent",
        &partial_agent_message_content(full_text, error, stop_reason),
        task_id,
    )
    .await
    {
        Ok(seq) => Some(seq),
        Err(e) => {
            tracing::error!(error = %e, "failed to persist partial agent message");
            None
        }
    }
}

fn partial_agent_message_content(
    full_text: &str,
    error: Option<&str>,
    stop_reason: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": "text",
        "text": full_text,
        "incomplete": true,
        "error": error,
        "stop_reason": stop_reason,
    })
}

fn classify_agent_failure_stop_reason(cancelled: bool) -> String {
    if cancelled {
        return "user_aborted".to_string();
    }
    "agent_error".to_string()
}

fn stop_reason_code(stop_reason: StopReason) -> &'static str {
    match stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::Other => "other",
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn publish_terminal_failure(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    full_text: &str,
    stop_reason: &str,
    error: &str,
) {
    let message_seq = persist_partial_agent_message(
        pool,
        user_id,
        session_id,
        full_text,
        Some(error),
        stop_reason,
        task_id,
    )
    .await;
    let payload = serde_json::json!({
        "message": error,
        "error": error,
        "stop_reason": stop_reason,
        "message_seq": message_seq,
        "incomplete": message_seq.is_some(),
    });
    publish_best_effort(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "agent_message_failed",
        payload.clone(),
    )
    .await;
    publish_best_effort(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "error",
        payload.clone(),
    )
    .await;
    publish_best_effort(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "agent_message_end",
        serde_json::json!({
            "stop_reason": stop_reason,
            "message_seq": message_seq,
            "incomplete": message_seq.is_some(),
            "error": error,
        }),
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn publish_best_effort(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    kind: &str,
    payload: serde_json::Value,
) {
    if let Err(e) =
        publish_and_persist(pool, user_id, session_id, task_id, event_bus, kind, payload).await
    {
        tracing::error!(kind, error = %e, "failed to persist terminal event");
    }
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
    let seq = match vault_events::insert(
        pool,
        user_id,
        session_id,
        task_id,
        kind,
        &payload,
        Some("main_agent"),
        ts,
    )
    .await
    {
        Ok(seq) => seq,
        Err(e) => {
            event_bus
                .publish(
                    session_id,
                    EventEnvelope {
                        seq: -1,
                        kind: kind.to_string(),
                        payload,
                        ts,
                    },
                )
                .await;
            return Err(e).with_context(|| format!("persisting event {kind}"));
        }
    };
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
    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use std::sync::Mutex;

    #[test]
    fn runtime_context_messages_prepends_handoff_as_user_context() {
        let summaries = vec!["旧对话摘要".to_string()];
        let messages = runtime_context_messages(&summaries);

        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0].role, Role::User));
        assert!(
            messages[0]
                .content
                .contains("COMPACTED PRIOR SESSION HISTORY")
        );
        assert!(messages[0].content.contains("旧对话摘要"));
    }

    #[test]
    fn cacheable_tool_filter_allows_only_low_recency_knowledge_tools() {
        assert!(is_cacheable_tool("corpus_search"));
        assert!(is_cacheable_tool("corpus_read"));
        assert!(is_cacheable_tool("get_company_info"));
        assert!(is_cacheable_tool("get_financials"));
        assert!(is_cacheable_tool("sec_filing_fetch"));
    }

    #[test]
    fn cacheable_tool_filter_rejects_realtime_or_stateful_tools() {
        assert!(!is_cacheable_tool("web_fetch"));
        assert!(!is_cacheable_tool("market_quote"));
        assert!(!is_cacheable_tool("tradingview_quote"));
        assert!(!is_cacheable_tool("get_a_share_market_snapshot"));
        assert!(!is_cacheable_tool("get_candlesticks"));
        assert!(!is_cacheable_tool("get_crypto_market"));
        assert!(!is_cacheable_tool("get_funding_rate"));
        assert!(!is_cacheable_tool("get_capital_flow"));
        assert!(!is_cacheable_tool("ask_user_question"));
        assert!(!is_cacheable_tool("update_plan"));
        assert!(!is_cacheable_tool("delegate_research"));
    }

    #[test]
    fn classify_tool_error_sets_kind_and_retryability() {
        let permission = classify_tool_error("API key not set");
        assert_eq!(permission.kind, "permission");
        assert!(!permission.retryable);

        let validation = classify_tool_error("invalid arguments JSON for web_fetch");
        assert_eq!(validation.kind, "validation");
        assert!(!validation.retryable);

        let transient = classify_tool_error("connection timeout from upstream");
        assert_eq!(transient.kind, "transient");
        assert!(transient.retryable);

        let unknown = classify_tool_error("provider returned a strange response");
        assert_eq!(unknown.kind, "unknown");
        assert!(unknown.retryable);
    }

    #[test]
    fn classify_tool_partial_status_marks_research_source_gaps() {
        assert_eq!(
            classify_tool_partial_status(
                "get_a_share_research_sources",
                "### 互动问答 · `irm_qa_sh` · unavailable\n- Source unavailable"
            ),
            Some("partial_with_unavailable_source")
        );
        assert_eq!(
            classify_tool_partial_status(
                "get_a_share_research_sources",
                "- No rows returned. This is a valid empty result"
            ),
            Some("success_with_valid_empty_sections")
        );
        assert_eq!(
            classify_tool_partial_status("market_quote", "unavailable"),
            None
        );
        assert_eq!(
            classify_tool_partial_status(
                "get_capital_flow",
                "[get_capital_flow: northbound disclosure unavailable after 2024-08-19]"
            ),
            Some("partial_with_unavailable_source")
        );
        assert_eq!(
            classify_tool_partial_status(
                "get_a_share_industry_context",
                "| Source unavailable | - | - | - | - | - | - | - | 行业资金流来源暂不可用；这不是资金流为零。 |"
            ),
            Some("partial_with_unavailable_source")
        );
    }

    #[test]
    fn tool_ui_artifact_preserves_full_output_outside_model_context() {
        let output = "## A股行情快照\n\n| 代码 | 最新价 |\n|---|---:|\n| 600519.SH | 1500 |\n";
        let artifact = build_tool_ui_artifact(
            "get_a_share_market_snapshot",
            r#"{"ts_codes":["600519.SH"]}"#,
            output,
        );
        assert_eq!(artifact["version"], 1);
        assert_eq!(artifact["tool_name"], "get_a_share_market_snapshot");
        assert_eq!(artifact["content_type"], "markdown");
        assert_eq!(artifact["arguments"]["ts_codes"][0], "600519.SH");
        assert_eq!(artifact["payload"]["markdown"], output);
    }

    #[test]
    fn tool_ui_artifact_keeps_json_payload_structured() {
        let artifact = build_tool_ui_artifact(
            "corpus_search",
            r#"{"query":"margin of safety"}"#,
            r#"{"hits":[{"id":"wikis/principles/concepts/margin-of-safety"}]}"#,
        );
        assert_eq!(artifact["content_type"], "json");
        assert_eq!(
            artifact["payload"]["hits"][0]["id"],
            "wikis/principles/concepts/margin-of-safety"
        );
    }

    #[test]
    fn compact_research_sources_keeps_sections_but_drops_urls() {
        let output = "## 600519.SH · A股研究源材料\n\n窗口：2025-11-24 → 2026-05-24\n\n### 公告 · `anns_d` · 77 rows\n- 2026-05-22 · 600519.SH · 标题一 · http://example.com/a\n- 2026-05-21 · 600519.SH · 标题二 · https://example.com/b\n- 2026-05-20 · 600519.SH · 标题三 · https://example.com/c\n";
        let compact = compact_tool_output_for_model("get_a_share_research_sources", output);
        assert!(compact.contains("## 600519.SH"));
        assert!(compact.contains("### 公告"));
        assert!(compact.contains("标题一"));
        assert!(compact.contains("标题二"));
        assert!(!compact.contains("标题三"));
        assert!(!compact.contains("http://example.com"));
        assert!(compact.contains("full source rows remain"));
    }

    #[test]
    fn compact_financials_keeps_latest_rows_and_field_metadata() {
        let output = "## 600519.SH · 利润表（近4期）\n\n| 报告期 | 营业总收入（亿） | 营业收入（亿） |\n|---|---:|---:|\n| 2026-03 | 547.03 | 539.09 |\n| 2025-12 | 1720.54 | 1688.38 |\n| 2025-09 | 1309.04 | 1284.54 |\n\n_字段口径: total_revenue=营业总收入；revenue=营业收入。_\n";
        let compact = compact_tool_output_for_model("get_financials", output);
        assert!(compact.contains("2026-03"));
        assert!(compact.contains("2025-12"));
        assert!(!compact.contains("2025-09"));
        assert!(compact.contains("total_revenue=营业总收入"));
        assert!(compact.contains("latest 2 rows"));
    }

    #[test]
    fn compact_industry_context_keeps_summary_and_limits_tables() {
        let output = "## A股行业上下文 · 600519.SH\n\n### 任务摘要\n- 标的：600519.SH\n- 主行业：L2 白酒 · 801123.SI\n- 行业指数近期表现：最新 2026-05-22 close=100 pct=1%\n- 资金面：东方财富盘中板块资金 2026-05-25 14:00 白酒 主力净流入=100 涨跌幅=1%\n\n### 申万分类\n| 层级 | 代码 | 名称 |\n|---|---|---|\n| L1 | 801120.SI | 食品饮料 |\n| L2 | 801123.SI | 白酒 |\n| L3 | 80112301.SI | 白酒III |\n\n### 同业样本\n| 代码 | 名称 |\n|---|---|\n| a | A |\n| b | B |\n| c | C |\n| d | D |\n| e | E |\n| f | F |\n| g | G |\n\n### 行业资金面\n| 来源 | 时间/日期 | 行业 | 代码 | 点位/收盘 | 涨跌幅% | 主力净流入 | 领涨股 | 说明 |\n|---|---|---|---|---:|---:|---:|---|---|\n| 东方财富盘中板块资金 | 2026-05-25 14:00 | 白酒 | BK0896 | 100 | 1 | 100 | A | 盘中 |\n| Tushare 日频行业资金 | 2026-05-22 | 白酒 | 801123.SI | 99 | 0.5 | 50 | B | 日频 |\n| extra | x | x | x | x | x | x | x | x |\n\n_Caveat: 行业分类和板块资金是 working model 的上下文，不是买卖信号。_";
        let compact = compact_tool_output_for_model("get_a_share_industry_context", output);
        assert!(compact.contains("主行业：L2 白酒"));
        assert!(compact.contains("东方财富盘中板块资金"));
        assert!(compact.contains("| f | F |"));
        assert!(!compact.contains("| g | G |"));
        assert!(!compact.contains("| extra |"));
        assert!(compact.contains("industry-context tables compacted"));
    }

    #[test]
    fn compact_generic_markdown_context_limits_bullets_and_tables() {
        let output = "## 中国宏观上下文\n\n### GDP · `cn_gdp` · 9 rows\n- q1\n- q2\n- q3\n- q4\n- q5\n\n### PMI\n| 月份 | PMI |\n|---|---:|\n| 1 | 50 |\n| 2 | 51 |\n| 3 | 52 |\n| 4 | 53 |\n| 5 | 54 |\n\n_Caveat: 宏观数据必须通过传导链连接到标的。_";
        let compact = compact_tool_output_for_model("get_china_macro_context", output);
        assert!(compact.contains("q4"));
        assert!(!compact.contains("q5"));
        assert!(compact.contains("| 4 | 53 |"));
        assert!(!compact.contains("| 5 | 54 |"));
        assert!(compact.contains("markdown context compacted"));
    }

    #[test]
    fn partial_agent_message_content_marks_incomplete_failure() {
        let content = partial_agent_message_content(
            "阶段性输出",
            Some("provider stream idle timeout after 90000ms"),
            "provider_stream_idle_timeout",
        );

        assert_eq!(content["type"], "text");
        assert_eq!(content["text"], "阶段性输出");
        assert_eq!(content["incomplete"], true);
        assert_eq!(
            content["error"],
            "provider stream idle timeout after 90000ms"
        );
        assert_eq!(content["stop_reason"], "provider_stream_idle_timeout");
    }

    #[test]
    fn classify_agent_failure_stop_reason_preserves_user_abort() {
        assert_eq!(classify_agent_failure_stop_reason(true), "user_aborted");
        assert_eq!(classify_agent_failure_stop_reason(false), "agent_error");
    }

    #[tokio::test]
    async fn publish_terminal_failure_persists_partial_message_and_terminal_events() {
        let pool = create_agent_test_pool().await;

        publish_terminal_failure(
            &pool,
            "u1",
            "s1",
            None,
            &EventBus::new(),
            "已完成的部分",
            "provider_stream_idle_timeout",
            "idle timeout",
        )
        .await;

        let content_json: String = sqlx::query_scalar(
            "SELECT content_json FROM messages WHERE user_id = 'u1' AND session_id = 's1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let content: serde_json::Value = serde_json::from_str(&content_json).unwrap();
        assert_eq!(content["text"], "已完成的部分");
        assert_eq!(content["incomplete"], true);
        assert_eq!(content["error"], "idle timeout");
        assert_eq!(content["stop_reason"], "provider_stream_idle_timeout");

        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM events WHERE user_id = 'u1' AND session_id = 's1' ORDER BY seq",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            kinds,
            vec!["agent_message_failed", "error", "agent_message_end"]
        );
    }

    #[tokio::test]
    async fn stream_eof_without_message_end_retries_and_does_not_commit_partial_attempt() {
        let pool = create_agent_test_pool().await;
        vault_messages::insert(
            &pool,
            "u1",
            "s1",
            "user",
            &serde_json::json!({ "type": "text", "text": "测试" }),
            None,
        )
        .await
        .unwrap();

        let provider = Arc::new(TruncatedThenCompleteProvider::default());
        run_chat_reply(
            pool.clone(),
            "u1".to_string(),
            "s1".to_string(),
            provider,
            EventBus::new(),
            CancellationToken::new(),
            ToolRegistry::empty(),
        )
        .await
        .unwrap();

        let messages = vault_messages::list(&pool, "u1", "s1", None, 20)
            .await
            .unwrap();
        let agent_message = messages
            .iter()
            .find(|row| row.role == "agent")
            .expect("agent message should be persisted");
        let content: serde_json::Value = serde_json::from_str(&agent_message.content_json).unwrap();
        assert_eq!(content["text"], "第二次完整输出");

        let rows = vault_events::list_for_session(&pool, "u1", "s1", None, None)
            .await
            .unwrap();
        assert!(rows.iter().any(|row| row.kind == "provider_retry"));
        assert!(rows.iter().any(|row| row.kind == "agent_message_reset"));
        let end = rows
            .iter()
            .rev()
            .find(|row| row.kind == "agent_message_end")
            .expect("terminal event should exist");
        let payload: serde_json::Value = serde_json::from_str(&end.payload_json).unwrap();
        assert_eq!(payload["stop_reason"], "end_turn");
    }

    #[derive(Default)]
    struct TruncatedThenCompleteProvider {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl LlmProvider for TruncatedThenCompleteProvider {
        fn name(&self) -> &str {
            "truncated-then-complete"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                return Ok(Box::pin(stream::iter(vec![Ok(LlmEvent::TextDelta {
                    text: "第一次不完整输出".to_string(),
                })])));
            }
            Ok(Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    text: "第二次完整输出".to_string(),
                }),
                Ok(LlmEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                }),
            ])))
        }
    }

    async fn create_agent_test_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for statement in [
            "CREATE TABLE messages (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                task_id TEXT,
                role TEXT NOT NULL,
                content_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, seq)
            )",
            "CREATE TABLE events (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                task_id TEXT,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                source TEXT,
                ts TEXT NOT NULL,
                persisted_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, seq)
            )",
            "CREATE TABLE agent_plan_items (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                task_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                step TEXT NOT NULL,
                status TEXT NOT NULL,
                evidence TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, task_id, item_id)
            )",
            "CREATE TABLE tool_call_runs (
                user_id TEXT NOT NULL,
                id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                task_id TEXT,
                invoker TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                arguments_json TEXT NOT NULL,
                result_json TEXT,
                success INTEGER,
                error TEXT,
                duration_ms INTEGER,
                started_at TEXT NOT NULL,
                completed_at TEXT,
                PRIMARY KEY (user_id, id)
            )",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool
    }

    #[test]
    fn tool_run_state_line_prefers_output_and_truncates_args() {
        let row = vault_tool_runs::ToolRunRow {
            id: "run-1".to_string(),
            tool_name: "corpus_search".to_string(),
            arguments_json: serde_json::json!({"query": "margin of safety"}).to_string(),
            result_json: Some(serde_json::json!({"output": "Buffett evidence"}).to_string()),
            error: None,
            completed_at: Some("2026-05-20T01:02:03Z".to_string()),
        };

        let line = format_tool_run_for_state(&row).unwrap();
        assert!(line.contains("`corpus_search`"));
        assert!(line.contains("(reusable)"));
        assert!(line.contains("margin of safety"));
        assert!(line.contains("Buffett evidence"));
    }

    #[test]
    fn tool_run_state_line_marks_market_quote_as_refresh_sensitive() {
        let row = vault_tool_runs::ToolRunRow {
            id: "run-quote".to_string(),
            tool_name: "market_quote".to_string(),
            arguments_json: serde_json::json!({"symbols": ["NVDA"], "market": "US"}).to_string(),
            result_json: Some(serde_json::json!({"output": "NVDA 133.70"}).to_string()),
            error: None,
            completed_at: Some("2026-05-20T01:02:03Z".to_string()),
        };

        let line = format_tool_run_for_state(&row).unwrap();
        assert!(line.contains("`market_quote`"));
        assert!(line.contains("(refresh-sensitive)"));
        assert!(line.contains("NVDA 133.70"));
    }

    #[test]
    fn tool_evidence_policy_separates_reusable_refresh_and_stateful_tools() {
        assert_eq!(tool_evidence_policy("get_financials"), "reusable");
        assert_eq!(tool_evidence_policy("market_quote"), "refresh-sensitive");
        assert_eq!(
            tool_evidence_policy("get_a_share_market_snapshot"),
            "refresh-sensitive"
        );
        assert_eq!(
            tool_evidence_policy("get_capital_flow"),
            "refresh-sensitive"
        );
        assert_eq!(
            tool_evidence_policy("get_crypto_market"),
            "refresh-sensitive"
        );
        assert_eq!(tool_evidence_policy("update_plan"), "stateful");
        assert_eq!(tool_evidence_policy("web_fetch"), "prior-evidence");
    }

    #[test]
    fn session_state_guidance_mentions_reuse_refresh_duplicate_and_search_budget() {
        assert!(tool_evidence_guidance().contains("reusable=low-recency"));
        assert!(tool_evidence_guidance().contains("refresh-sensitive=market/quote/capital"));
        assert!(web_search_budget_guidance().contains("avoid reopening the same URL/PDF"));
        assert!(web_search_budget_guidance().contains("stop once the evidence is enough"));
    }

    #[test]
    fn tool_run_state_line_shows_error_when_present() {
        let row = vault_tool_runs::ToolRunRow {
            id: "run-2".to_string(),
            tool_name: "web_fetch".to_string(),
            arguments_json: "{}".to_string(),
            result_json: Some(serde_json::json!({"output": "ignored"}).to_string()),
            error: Some("permission denied".to_string()),
            completed_at: None,
        };

        let line = format_tool_run_for_state(&row).unwrap();
        assert!(line.contains("unknown-time"));
        assert!(line.contains("ERROR permission denied"));
        assert!(!line.contains("ignored"));
    }

    #[test]
    fn web_search_state_line_uses_detail_and_sources() {
        let row = vault_events::EventRow {
            seq: 7,
            task_id: None,
            kind: "web_search_call".to_string(),
            payload_json: serde_json::json!({
                "action": "search",
                "detail": "OpenAI latest filings",
                "sources": ["https://example.com/a", "https://example.com/b"]
            })
            .to_string(),
            source: Some("main_agent".to_string()),
            ts: "2026-05-20T01:02:03Z".to_string(),
        };

        let line = format_web_search_for_state(&row).unwrap();
        assert!(line.contains("`search`"));
        assert!(line.contains("OpenAI latest filings"));
        assert!(line.contains("https://example.com/a"));
    }

    #[test]
    fn recent_web_search_guard_mentions_reuse_and_existing_pdf() {
        let rows = vec![
            vault_events::EventRow {
                seq: 7,
                task_id: None,
                kind: "web_search_call".to_string(),
                payload_json: serde_json::json!({
                    "status": "completed",
                    "action": "open_page",
                    "detail": "https://example.com/report.pdf",
                })
                .to_string(),
                source: Some("main_agent".to_string()),
                ts: "2026-05-20T01:02:03Z".to_string(),
            },
            vault_events::EventRow {
                seq: 8,
                task_id: None,
                kind: "web_search_call".to_string(),
                payload_json: serde_json::json!({
                    "status": "in_progress",
                    "action": "open_page",
                    "detail": "https://example.com/ignored.pdf",
                })
                .to_string(),
                source: Some("main_agent".to_string()),
                ts: "2026-05-20T01:02:04Z".to_string(),
            },
        ];

        let input = recent_web_search_guard_input_from_events(&rows).unwrap();
        let content = input["content"].as_str().unwrap();
        assert!(content.contains("Do not reopen"));
        assert!(content.contains("https://example.com/report.pdf"));
        assert!(!content.contains("ignored.pdf"));
    }

    #[test]
    fn active_plan_state_guidance_warns_before_closing_with_open_items() {
        let items = vec![
            vault_plans::PlanItemRow {
                item_id: "p1".to_string(),
                seq: 1,
                step: "collect evidence".to_string(),
                status: "completed".to_string(),
                evidence: None,
            },
            vault_plans::PlanItemRow {
                item_id: "p2".to_string(),
                seq: 2,
                step: "finalize answer".to_string(),
                status: "in_progress".to_string(),
                evidence: None,
            },
        ];

        let guidance = format_active_plan_state_guidance(&items);
        assert!(guidance.contains("before a final/closing answer"));
        assert!(guidance.contains("do not leave fake in_progress/pending"));
    }

    #[test]
    fn active_plan_state_guidance_allows_completed_plan_to_close() {
        let items = vec![vault_plans::PlanItemRow {
            item_id: "p1".to_string(),
            seq: 1,
            step: "collect evidence".to_string(),
            status: "completed".to_string(),
            evidence: None,
        }];

        let guidance = format_active_plan_state_guidance(&items);
        assert!(guidance.contains("all items are completed"));
        assert!(guidance.contains("synthesize the answer"));
    }

    #[test]
    fn plan_reminder_is_interval_based() {
        assert!(plan_reminder_tone(1, 0, None).is_none());
        assert!(matches!(
            plan_reminder_tone(4, 0, None),
            Some(PlanReminderTone::Soft)
        ));
        assert!(matches!(
            plan_reminder_tone(8, 0, None),
            Some(PlanReminderTone::Firm)
        ));
        assert!(matches!(
            plan_reminder_tone(12, 0, None),
            Some(PlanReminderTone::Strong)
        ));
        assert!(plan_reminder_tone(12, 0, Some(12)).is_none());
    }
}
