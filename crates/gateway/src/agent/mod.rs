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

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio::time::{sleep, timeout, Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::events::{EventBus, EventEnvelope};
use crate::llm::{
    ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role, StopReason, ToolSpec, Usage,
    WebSearchAction,
};
use crate::vault::{
    events as vault_events, messages as vault_messages, plans as vault_plans,
    sessions as vault_sessions, tool_runs as vault_tool_runs,
};

use tools::{ToolContext, ToolRegistry};

pub(crate) const DEFAULT_MODEL: &str = "gpt-5.5";

/// Hard cap on tool-call iterations within a single turn (one agent loop).
const MAX_TOOL_ITERATIONS: usize = 24;
const MAX_PROVIDER_RETRIES: usize = 10;
const PROVIDER_RETRY_BASE_MS: u64 = 1_000;
const PROVIDER_RETRY_MAX_MS: u64 = 30_000;
const PROVIDER_STREAM_IDLE_TIMEOUT_MS: u64 = 90_000;
const PROVIDER_SYNTHESIS_TIMEOUT_MS: u64 = 90_000;
const PLAN_REMINDER_INTERVAL_ITERATIONS: usize = 4;
// Streaming deltas are broadcast to subscribers as they arrive (coalescing only
// sub-frame bursts) so chat reads token-by-token. They are never persisted —
// history replay rebuilds the bubble from the final messages row, not delta
// events — so the prior per-flush SQLite write was pure churn. The window caps
// the SSE frame rate (~25fps) to keep the broadcast channel from lagging while
// still feeling continuous; the frontend typewriter smooths the rest.
const AGENT_DELTA_STREAM_CHARS: usize = 240;
const AGENT_DELTA_STREAM_MS: u64 = 40;

pub(crate) fn prompt_cache_key_for(model: &str, session_id: &str, invoker: &str) -> String {
    let mode = std::env::var("LEEK_PROMPT_CACHE_KEY_MODE")
        .unwrap_or_else(|_| "session".to_string())
        .to_ascii_lowercase();
    if mode == "global" {
        return format!("leek:{}:{}", cache_key_part(model), cache_key_part(invoker));
    }
    if mode == "legacy_global" {
        return format!("leek:{}:main-agent", cache_key_part(model));
    }
    format!(
        "leek:{}:session:{}:{}",
        cache_key_part(model),
        cache_key_part(session_id),
        cache_key_part(invoker)
    )
}

fn cache_key_part(raw: &str) -> String {
    let part = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':') {
                c
            } else {
                '-'
            }
        })
        .take(80)
        .collect::<String>();
    if part.is_empty() {
        "default".to_string()
    } else {
        part
    }
}

#[derive(Clone, Copy, Default)]
struct ReplayInputStats {
    total_items: usize,
    reasoning_items: usize,
    function_call_items: usize,
    function_output_items: usize,
}

fn replay_input_stats(items: &[serde_json::Value]) -> ReplayInputStats {
    let mut stats = ReplayInputStats {
        total_items: items.len(),
        ..ReplayInputStats::default()
    };
    for item in items {
        match item.get("type").and_then(|t| t.as_str()) {
            Some("reasoning") => stats.reasoning_items += 1,
            Some("function_call") => stats.function_call_items += 1,
            Some("function_call_output") => stats.function_output_items += 1,
            _ => {}
        }
    }
    stats
}

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
    // Rebuild the append-only conversation queue: the compaction-summary
    // boundary plus every user/agent/tool row after it, expanded into raw
    // Responses API items. This `replay_inputs` is the stable prefix — within a
    // turn it only grows at the tail (echoed function_call + function_call_output
    // after each tool), so the provider's prompt cache hits byte-for-byte.
    let (mut replay_inputs, handoff_summaries) =
        rebuild_replay(&pool, &user_id, &session_id).await?;

    if replay_inputs.is_empty() && handoff_summaries.is_empty() {
        anyhow::bail!("run_chat_reply called with no replayable history in session");
    }

    // Only the compaction summary rides in `messages` (prepended as runtime
    // context); the full user/agent/tool transcript replays via replay_inputs.
    // `mut` because a mid-turn compaction swaps in a fresh summary.
    let mut messages = runtime_context_messages(&handoff_summaries);
    let system_prompt = harness::build_system_prompt();
    // Experimental only. On the codex backend, replaying encrypted reasoning
    // reduced cache hit rate in long tool-loop probes versus replaying only
    // user/function_call/function_call_output items.
    let replay_reasoning = std::env::var("LEEK_REPLAY_REASONING")
        .map(|v| v != "0")
        .unwrap_or(false);
    let persist_reasoning = std::env::var("LEEK_PERSIST_REASONING")
        .map(|v| v != "0")
        .unwrap_or(false);

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

    let mut full_text = String::new();
    let mut final_text = String::new();
    let mut stop_reason = "end_turn".to_string();
    let mut plan_last_update_iteration = 0usize;
    let mut plan_last_reminder_iteration: Option<usize> = None;
    let mut iteration = 0usize;
    let mut last_input_tokens: i64 = 0;
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

        'iterations: loop {
        // Mid-turn auto-compaction (codex parity): one long turn can blow the
        // context window on tool outputs alone, between two user messages.
        // When the last provider call already reported input_tokens past the
        // model's threshold, compact the queue in-place and rebuild the replay.
        // This MUST take the internal path — the in-flight reply already holds
        // the active_replies slot, so going through start_compaction would
        // self-deadlock on that lock.
        if last_input_tokens >= crate::llm::model_limits::auto_compact_threshold(DEFAULT_MODEL) {
            compact_session_tail(&pool, &user_id, &session_id, &event_bus, provider.clone()).await?;
            let (rebuilt, summaries) = rebuild_replay(&pool, &user_id, &session_id).await?;
            replay_inputs = rebuilt;
            messages = runtime_context_messages(&summaries);
            last_input_tokens = 0;
        }

        if iteration >= MAX_TOOL_ITERATIONS {
            stop_reason = "max_tool_turns_finalized".to_string();
            match finalize_after_tool_budget(
                &session_id,
                &event_bus,
                provider.clone(),
                &messages,
                &system_prompt,
                &replay_inputs,
                cancel.clone(),
            )
            .await
            {
                Ok(text) if !text.trim().is_empty() => {
                    final_text = text;
                }
                Ok(_) | Err(_) => {
                    final_text = format!(
                        "我已经达到本轮工具调用上限（{MAX_TOOL_ITERATIONS} 轮），先交付当前阶段性结果：{}\n\n后续需要继续补齐尚未验证的证据，再形成最终判断。",
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
            break 'iterations;
        }

        let mut pending_calls: Vec<PendingCall> = Vec::new();
        let mut pending_reasoning: Vec<serde_json::Value> = Vec::new();
        let mut turn_text = String::new();
        let mut narration_buffer = String::new();
        let mut delta_buffer = String::new();
        let mut provider_retries = 0usize;

        loop {
            pending_calls.clear();
            pending_reasoning.clear();
            turn_text.clear();
            narration_buffer.clear();
            delta_buffer.clear();
            let mut last_delta_flush = Instant::now();
            // Stable prefix: the append-only replay. The only thing that may be
            // appended at the tail is a (throttled) plan reminder, kept last so
            // the cached prefix never shifts.
            let mut request_inputs = replay_inputs.clone();
            if let Some(plan_input) = active_plan_reminder_input(
                &pool,
                &user_id,
                &session_id,
                iteration,
                plan_last_update_iteration,
                &mut plan_last_reminder_iteration,
            )
            .await?
            {
                request_inputs.push(plan_input);
            }
            let request_input_stats = replay_input_stats(&request_inputs);
            let request_model = DEFAULT_MODEL.to_string();
            let request_prompt_cache_key =
                prompt_cache_key_for(&request_model, &session_id, "main_agent");
            let request_started_at = chrono::Utc::now();
            let request_started = Instant::now();
            let req = ChatRequest {
                messages: messages.clone(),
                system: Some(system_prompt.clone()),
                model: request_model.clone(),
                session_id: Some(session_id.clone()),
                prompt_cache_key: Some(request_prompt_cache_key.clone()),
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
                            break 'iterations;
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
                            break 'iterations;
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
            if provider_retries > 0 {
                publish_provider_recovered(
                    &pool,
                    &user_id,
                    &session_id,
                    None,
                    &event_bus,
                    provider.name(),
                    provider_retries,
                )
                .await?;
            }

            let full_text_len_before_attempt = full_text.len();
            let mut stream_error: Option<anyhow::Error> = None;
            let mut saw_message_end = false;
            'stream: loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        stop_reason = "user_aborted".to_string();
                        fatal_error = Some("user_aborted".to_string());
                        break 'iterations;
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
                                delta_buffer.push_str(&text);
                                if delta_buffer.len() >= AGENT_DELTA_STREAM_CHARS ||
                                    last_delta_flush.elapsed() >= Duration::from_millis(AGENT_DELTA_STREAM_MS)
                                {
                                    broadcast_agent_message_delta(&session_id, &event_bus, &mut delta_buffer).await;
                                    last_delta_flush = Instant::now();
                                }
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
                                    iteration,
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
                            Ok(LlmEvent::Reasoning { encrypted_content, summary }) => {
                                // Held until we know the model continued the loop
                                // (had function_calls); then mirrored onto the
                                // replay tail before those calls. `id` is omitted
                                // — codex drops it on replay (it names a server
                                // item absent under store:false).
                                pending_reasoning.push(serde_json::json!({
                                    "type": "reasoning",
                                    "summary": summary,
                                    "encrypted_content": encrypted_content,
                                }));
                            }
                            Ok(LlmEvent::Usage(u)) => {
                                // Feeds the mid-turn auto-compaction check at the
                                // top of the next iteration.
                                last_input_tokens = i64::from(u.input_tokens);
                                let uncached_input_tokens =
                                    u.input_tokens.saturating_sub(u.cache_read_tokens);
                                let cache_hit_rate = if u.input_tokens == 0 {
                                    0.0
                                } else {
                                    f64::from(u.cache_read_tokens)
                                        / f64::from(u.input_tokens)
                                };
                                record_llm_usage(
                                    &pool,
                                    &user_id,
                                    &session_id,
                                    provider.name(),
                                    &request_model,
                                    &u,
                                    request_started.elapsed(),
                                    &request_started_at.to_rfc3339(),
                                )
                                .await?;
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
                                        "cache_write_tokens": u.cache_write_tokens,
                                        "uncached_input_tokens": uncached_input_tokens,
                                        "cache_hit_rate": cache_hit_rate,
                                        "prompt_cache_key": request_prompt_cache_key,
                                        "input_item_count": request_input_stats.total_items,
                                        "reasoning_item_count": request_input_stats.reasoning_items,
                                        "function_call_item_count": request_input_stats.function_call_items,
                                        "function_output_item_count": request_input_stats.function_output_items,
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

            broadcast_agent_message_delta(&session_id, &event_bus, &mut delta_buffer).await;

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
                    break 'iterations;
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
            if replay_reasoning && persist_reasoning {
                persist_reasoning_items(&pool, &user_id, &session_id, &pending_reasoning).await?;
            }
            final_text = turn_text.clone();
            break 'iterations;
        }

        reset_agent_message_candidate(&pool, &user_id, &session_id, None, &event_bus, &turn_text)
            .await?;
        flush_agent_narration(
            &pool,
            &user_id,
            &session_id,
            None,
            &event_bus,
            iteration,
            &mut narration_buffer,
        )
        .await?;

        publish_agent_trace_note(
            &pool,
            &user_id,
            &session_id,
            None,
            &event_bus,
            iteration,
            &format_tool_batch_trace(&pending_calls),
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

        // Persist this iteration's whole batch of function_calls as one
        // append-only `assistant_tool_calls` row, and mirror them onto the
        // replay tail so the codex-required "call precedes output" ordering is
        // preserved (all calls, then all outputs).
        if replay_reasoning && persist_reasoning {
            persist_reasoning_items(&pool, &user_id, &session_id, &pending_reasoning).await?;
        }
        // This iteration's reasoning items ride the replay tail just before their
        // function_calls (codex input ordering), so the backend's prompt cache
        // keeps the chain-of-thought prefix across the tool loop instead of
        // breaking at every post-reasoning step.
        if replay_reasoning {
            replay_inputs.append(&mut pending_reasoning);
        } else {
            pending_reasoning.clear();
        }
        persist_tool_calls(&pool, &user_id, &session_id, &pending_calls).await?;
        for call in &pending_calls {
            replay_inputs.push(serde_json::json!({
                "type": "function_call",
                "call_id": call.call_id,
                "name": call.name,
                "arguments": call.arguments,
            }));
        }

        // Execute pending tools sequentially (parallelism can come later;
        // most tools we'll ship are I/O bound so order rarely matters but
        // serializing keeps the audit trail simple).
        let mut user_question: Option<serde_json::Value> = None;
        for call in &pending_calls {
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
                plan_last_update_iteration = iteration + 1;
                plan_last_reminder_iteration = None;
            }
            let duration_ms = tool_started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(i64::MAX);
            let partial_status = classify_tool_partial_status(&call.name, &output_str);
            // What the model sees on replay == what we persist into the queue:
            // the raw tool output, only ever byte-capped (never semantically
            // rewritten). The full untruncated output stays in tool_call_runs.
            let queue_output = cap_tool_output(&output_str);
            let ui_artifact = build_tool_ui_artifact(&call.name, &call.arguments, &output_str);
            let result_json = serde_json::json!({
                "output": output_str,
                "output_bytes": output_str.len(),
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

            // Persist the function_call_output (capped raw) as an append-only
            // `tool_result` row and mirror it onto the replay tail. The matching
            // function_call was already echoed before the dispatch loop.
            persist_tool_result(&pool, &user_id, &session_id, &call.call_id, &queue_output).await?;
            replay_inputs.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call.call_id,
                "output": queue_output,
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
            let mut clarification_delta = final_text.clone();
            broadcast_agent_message_delta(&session_id, &event_bus, &mut clarification_delta).await;
            break 'iterations;
        }

        iteration += 1;
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
        touch_session_best_effort(&pool, &user_id, &session_id).await;

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

/// Read the session's append-only queue and rebuild the Responses API replay:
/// the compaction-summary boundary (returned separately for runtime-context
/// injection) plus every user/agent/tool row after it, expanded into raw items,
/// with orphan function_calls dropped.
async fn rebuild_replay(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<(Vec<serde_json::Value>, Vec<String>)> {
    let all_history = vault_messages::list(pool, user_id, session_id, None, 1000).await?;

    // Pre-compaction rows stay in the DB (read-only in the UI) but never enter
    // LLM context; the latest summary is injected separately as runtime context.
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

    let mut replay_inputs: Vec<serde_json::Value> = Vec::new();
    for row in &all_history[tail_start..] {
        replay_inputs.extend(vault_messages::row_to_input_items(
            &row.role,
            &row.content_json,
        ));
    }
    drop_orphan_function_calls(&mut replay_inputs);
    Ok((replay_inputs, handoff_summaries))
}

/// Drop any `function_call` whose `function_call_output` is missing (e.g. a
/// crash between persisting the call batch and its results). The codex backend
/// rejects orphan call_ids, so we never replay them.
fn drop_orphan_function_calls(replay_inputs: &mut Vec<serde_json::Value>) {
    let answered: std::collections::HashSet<String> = replay_inputs
        .iter()
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("function_call_output"))
        .filter_map(|v| v.get("call_id").and_then(|c| c.as_str()).map(String::from))
        .collect();
    replay_inputs.retain(|v| {
        if v.get("type").and_then(|t| t.as_str()) == Some("function_call") {
            v.get("call_id")
                .and_then(|c| c.as_str())
                .map(|id| answered.contains(id))
                .unwrap_or(false)
        } else {
            true
        }
    });
}

/// Persist one iteration's batch of function_calls as a single
/// `assistant_tool_calls` queue row. Stores the model's raw `arguments` string
/// verbatim (no re-serialization) so the replay is byte-identical.
async fn persist_tool_calls(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    calls: &[PendingCall],
) -> Result<i64> {
    let items: Vec<serde_json::Value> = calls
        .iter()
        .map(|c| {
            serde_json::json!({
                "type": "function_call",
                "call_id": c.call_id,
                "name": c.name,
                "arguments": c.arguments,
            })
        })
        .collect();
    vault_messages::insert(
        pool,
        user_id,
        session_id,
        "assistant_tool_calls",
        &serde_json::json!({ "type": "tool_calls", "items": items }),
        None,
    )
    .await
}

async fn persist_reasoning_items(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    items: &[serde_json::Value],
) -> Result<Option<i64>> {
    if items.is_empty() {
        return Ok(None);
    }
    let seq = vault_messages::insert(
        pool,
        user_id,
        session_id,
        "assistant_reasoning",
        &serde_json::json!({ "type": "reasoning_items", "items": items }),
        None,
    )
    .await?;
    Ok(Some(seq))
}

/// Persist one function_call_output as a `tool_result` queue row. `output` is
/// the byte-capped raw tool output (see `cap_tool_output`).
async fn persist_tool_result(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    call_id: &str,
    output: &str,
) -> Result<i64> {
    vault_messages::insert(
        pool,
        user_id,
        session_id,
        "tool_result",
        &serde_json::json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": output,
        }),
        None,
    )
    .await
}

const TOOL_OUTPUT_BYTE_LIMIT_DEFAULT: usize = 24_000;

fn tool_output_byte_limit() -> usize {
    std::env::var("LEEK_TOOL_OUTPUT_BYTE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(TOOL_OUTPUT_BYTE_LIMIT_DEFAULT)
}

/// codex-style backstop: cap a single tool output before it enters context.
/// This is a byte truncation (keeps the head verbatim, flags the cut), never a
/// semantic rewrite — the model knows there is more and how to get it. Per-tool
/// "return the right amount" logic lives in each tool's implementation; this
/// only guards against a tool that forgot to bound itself.
fn cap_tool_output(output: &str) -> String {
    let limit = tool_output_byte_limit();
    if output.len() <= limit {
        return output.to_string();
    }
    format!(
        "{}\n\n[输出已截断至 {} 字节；完整版在 vault.tool_call_runs / 前端卡片，或用更窄的参数重新查询。]",
        preview(output, limit),
        limit
    )
}

/// Internal mid-turn compaction: summarize the live queue tail and append a
/// `compaction_summary` row, WITHOUT going through `active_replies` (the calling
/// reply already holds that slot). Emits compaction.started/completed so the UI
/// reflects it.
async fn compact_session_tail(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    event_bus: &EventBus,
    provider: Arc<dyn LlmProvider>,
) -> Result<()> {
    publish_and_persist(
        pool,
        user_id,
        session_id,
        None,
        event_bus,
        "compaction.started",
        serde_json::json!({ "trigger": "auto_mid_turn", "focus": serde_json::Value::Null }),
    )
    .await?;

    let all_history = vault_messages::list(pool, user_id, session_id, None, 1000).await?;
    let start_idx = all_history
        .iter()
        .rposition(|r| r.role == "compaction_summary")
        .map(|i| i + 1)
        .unwrap_or(0);
    let history = &all_history[start_idx..];
    if history.is_empty() {
        anyhow::bail!("mid-turn compaction: no messages since last compaction");
    }
    let messages_removed = history.len() as i64;

    let summary =
        compact::summarize_session(provider, history, None, CancellationToken::new()).await?;

    vault_messages::insert(
        pool,
        user_id,
        session_id,
        "compaction_summary",
        &serde_json::json!({ "type": "text", "text": summary.clone() }),
        None,
    )
    .await?;

    publish_and_persist(
        pool,
        user_id,
        session_id,
        None,
        event_bus,
        "compaction.completed",
        serde_json::json!({
            "summary_md": summary,
            "messages_removed": messages_removed,
            "messages_retained": 1,
            "trigger": "auto_mid_turn",
        }),
    )
    .await?;
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

fn build_tool_ui_artifact(name: &str, arguments_json: &str, output: &str) -> serde_json::Value {
    let arguments = serde_json::from_str::<serde_json::Value>(arguments_json)
        .unwrap_or(serde_json::Value::Null);
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

fn tool_output_preview_limit(name: &str) -> usize {
    match name {
        "get_a_share_industry_context" => 6000,
        _ => 2000,
    }
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
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_after_tool_budget(
    session_id: &str,
    event_bus: &EventBus,
    provider: Arc<dyn LlmProvider>,
    messages: &[ChatMessage],
    system_prompt: &str,
    replay_inputs: &[serde_json::Value],
    cancel: CancellationToken,
) -> Result<String> {
    let mut request_inputs = replay_inputs.to_vec();
    request_inputs.push(serde_json::json!({
        "role": "user",
        "content": format!(
            "Runtime note: this turn reached the tool-call budget ({MAX_TOOL_ITERATIONS}). \
             Do not call more tools. Give a concise Chinese partial answer using \
             only evidence already in context: what is established, what is still \
             missing, and what should happen next."
        ),
    }));
    let req = ChatRequest {
        messages: messages.to_vec(),
        system: Some(system_prompt.to_string()),
        model: DEFAULT_MODEL.to_string(),
        session_id: Some(session_id.to_string()),
        prompt_cache_key: Some(prompt_cache_key_for(
            DEFAULT_MODEL,
            session_id,
            "main_agent:finalize",
        )),
        max_output_tokens: None,
        tools: Vec::new(),
        additional_inputs: request_inputs,
        reasoning_effort: None,
    };
    let mut stream = provider.chat(req).await?;
    let mut text_out = String::new();
    let mut delta_buffer = String::new();
    let mut last_delta_flush = Instant::now();
    while let Some(evt) = stream.next().await {
        if cancel.is_cancelled() {
            anyhow::bail!("user_aborted");
        }
        match evt? {
            LlmEvent::TextDelta { text } => {
                text_out.push_str(&text);
                delta_buffer.push_str(&text);
                if delta_buffer.len() >= AGENT_DELTA_STREAM_CHARS
                    || last_delta_flush.elapsed() >= Duration::from_millis(AGENT_DELTA_STREAM_MS)
                {
                    broadcast_agent_message_delta(session_id, event_bus, &mut delta_buffer).await;
                    last_delta_flush = Instant::now();
                }
            }
            LlmEvent::MessageEnd { .. } => break,
            _ => {}
        }
    }
    broadcast_agent_message_delta(session_id, event_bus, &mut delta_buffer).await;
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
    iteration: usize,
    plan_last_update_iteration: usize,
    plan_last_reminder_iteration: &mut Option<usize>,
) -> Result<Option<serde_json::Value>> {
    let Some(tone) = plan_reminder_tone(
        iteration,
        plan_last_update_iteration,
        *plan_last_reminder_iteration,
    ) else {
        return Ok(None);
    };
    let items = vault_plans::list_current(pool, user_id, session_id, None).await?;
    if items.is_empty() {
        return Ok(None);
    }
    if items.iter().all(|item| item.status == "completed") {
        return Ok(None);
    }
    *plan_last_reminder_iteration = Some(iteration);
    Ok(Some(serde_json::json!({
        "role": "user",
        "content": format_plan_reminder(tone, &items),
    })))
}

fn plan_reminder_tone(
    iteration: usize,
    plan_last_update_iteration: usize,
    plan_last_reminder_iteration: Option<usize>,
) -> Option<PlanReminderTone> {
    if iteration <= plan_last_update_iteration {
        return None;
    }
    let elapsed = iteration - plan_last_update_iteration;
    if elapsed < PLAN_REMINDER_INTERVAL_ITERATIONS
        || elapsed % PLAN_REMINDER_INTERVAL_ITERATIONS != 0
    {
        return None;
    }
    if plan_last_reminder_iteration == Some(iteration) {
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

fn format_tool_batch_trace(calls: &[PendingCall]) -> String {
    let mut names = calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    let shown = names.iter().take(5).copied().collect::<Vec<_>>().join(", ");
    if names.len() > 5 {
        format!("准备调用 {} 个工具补齐证据：{} 等。", calls.len(), shown)
    } else {
        format!("准备调用 {} 个工具补齐证据：{}。", calls.len(), shown)
    }
}

#[allow(clippy::too_many_arguments)]
async fn publish_agent_trace_note(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    iteration: usize,
    text: &str,
) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    publish_and_persist(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "agent_narration",
        // Payload key stays "turn" to preserve the SSE contract the frontend
        // matches on; the value is the iteration counter.
        serde_json::json!({
            "turn": iteration,
            "text": text,
            "source": "runtime_trace",
        }),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_llm_usage(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    provider_name: &str,
    model: &str,
    usage: &Usage,
    duration: Duration,
    started_at: &str,
) -> Result<()> {
    let duration_ms = duration.as_millis().try_into().unwrap_or(i64::MAX);
    sqlx::query(
        "INSERT INTO llm_usage_log \
         (user_id, id, provider_name, model, invoker, session_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, duration_ms, success, started_at) \
         VALUES (?, ?, ?, ?, 'main_agent', ?, ?, ?, ?, ?, ?, 1, ?)",
    )
    .bind(user_id)
    .bind(Uuid::now_v7().to_string())
    .bind(provider_name)
    .bind(model)
    .bind(session_id)
    .bind(i64::from(usage.input_tokens))
    .bind(i64::from(usage.output_tokens))
    .bind(i64::from(usage.cache_read_tokens))
    .bind(i64::from(usage.cache_write_tokens))
    .bind(duration_ms)
    .bind(started_at)
    .execute(pool)
    .await
    .context("recording llm usage")?;
    Ok(())
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
async fn publish_provider_recovered(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    event_bus: &EventBus,
    provider: &str,
    retries: usize,
) -> Result<()> {
    publish_and_persist(
        pool,
        user_id,
        session_id,
        task_id,
        event_bus,
        "provider_recovered",
        serde_json::json!({
            "provider": provider,
            "retries": retries,
            "message": "provider recovered; continuing current turn",
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

async fn touch_session_best_effort(pool: &SqlitePool, user_id: &str, session_id: &str) {
    if let Err(e) = vault_sessions::touch(pool, user_id, session_id).await {
        tracing::warn!(session_id, error = %e, "failed to touch session after agent reply");
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
    iteration: usize,
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
        // Payload key stays "turn" for the SSE contract; value is the iteration.
        serde_json::json!({
            "turn": iteration,
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

// Broadcast-only: a streaming delta goes straight to subscribers and is never
// written to vault.events. seq is -1 because the frontend ignores the SSE id for
// deltas — they merge by append, not by seq — and the authoritative text is the
// final messages row persisted at turn end.
async fn broadcast_agent_message_delta(
    session_id: &str,
    event_bus: &EventBus,
    buffer: &mut String,
) {
    if buffer.is_empty() {
        return;
    }
    let text = std::mem::take(buffer);
    event_bus
        .publish(
            session_id,
            EventEnvelope {
                seq: -1,
                kind: "agent_message_delta".to_string(),
                payload: serde_json::json!({ "text": text }),
                ts: chrono::Utc::now(),
            },
        )
        .await;
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
        assert!(messages[0]
            .content
            .contains("COMPACTED PRIOR SESSION HISTORY"));
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
        assert!(rows.iter().any(|row| row.kind == "provider_recovered"));
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

    #[test]
    fn drop_orphan_function_calls_removes_unanswered_calls() {
        let mut inputs = vec![
            serde_json::json!({"role":"user","content":"q"}),
            serde_json::json!({"type":"function_call","call_id":"c1","name":"t","arguments":"{}"}),
            serde_json::json!({"type":"function_call","call_id":"c2","name":"t","arguments":"{}"}),
            serde_json::json!({"type":"function_call_output","call_id":"c1","output":"ok"}),
        ];
        drop_orphan_function_calls(&mut inputs);
        // c2 has no matching output → dropped; c1 (answered) and the user msg stay.
        assert_eq!(inputs.len(), 3);
        assert!(inputs
            .iter()
            .any(|v| v.get("call_id").and_then(|c| c.as_str()) == Some("c1")
                && v.get("type").and_then(|t| t.as_str()) == Some("function_call")));
        assert!(!inputs
            .iter()
            .any(|v| v.get("call_id").and_then(|c| c.as_str()) == Some("c2")));
    }

    #[derive(Default)]
    struct ToolThenAnswerProvider {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl LlmProvider for ToolThenAnswerProvider {
        fn name(&self) -> &str {
            "tool-then-answer"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                return Ok(Box::pin(stream::iter(vec![
                    Ok(LlmEvent::Reasoning {
                        encrypted_content: "ENC_TOOL".to_string(),
                        summary: serde_json::json!([]),
                    }),
                    Ok(LlmEvent::FunctionCall {
                        call_id: "c1".to_string(),
                        name: "get_financials".to_string(),
                        arguments: "{\"ts_code\":\"600519\"}".to_string(),
                    }),
                    Ok(LlmEvent::MessageEnd {
                        stop_reason: StopReason::EndTurn,
                    }),
                ])));
            }
            Ok(Box::pin(stream::iter(vec![
                Ok(LlmEvent::Reasoning {
                    encrypted_content: "ENC_FINAL".to_string(),
                    summary: serde_json::json!([]),
                }),
                Ok(LlmEvent::TextDelta {
                    text: "最终答案".to_string(),
                }),
                Ok(LlmEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                }),
            ])))
        }
    }

    #[tokio::test]
    async fn dispatch_persists_tool_trace_and_replays_append_only() {
        let pool = create_agent_test_pool().await;
        vault_messages::insert(
            &pool,
            "u1",
            "s1",
            "user",
            &serde_json::json!({ "type": "text", "text": "查茅台财报" }),
            None,
        )
        .await
        .unwrap();

        run_chat_reply(
            pool.clone(),
            "u1".to_string(),
            "s1".to_string(),
            Arc::new(ToolThenAnswerProvider::default()),
            EventBus::new(),
            CancellationToken::new(),
            ToolRegistry::empty(),
        )
        .await
        .unwrap();

        let rows = vault_messages::list(&pool, "u1", "s1", None, 100)
            .await
            .unwrap();
        let roles: Vec<&str> = rows.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(
            roles,
            vec!["user", "assistant_tool_calls", "tool_result", "agent"]
        );

        let calls_row = rows
            .iter()
            .find(|r| r.role == "assistant_tool_calls")
            .unwrap();
        let calls_json: serde_json::Value = serde_json::from_str(&calls_row.content_json).unwrap();
        assert_eq!(calls_json["items"][0]["call_id"], "c1");
        assert_eq!(calls_json["items"][0]["name"], "get_financials");
        assert_eq!(
            calls_json["items"][0]["arguments"],
            "{\"ts_code\":\"600519\"}"
        );

        let result_row = rows.iter().find(|r| r.role == "tool_result").unwrap();
        let result_json: serde_json::Value =
            serde_json::from_str(&result_row.content_json).unwrap();
        assert_eq!(result_json["call_id"], "c1");
        assert!(result_row.seq > calls_row.seq);

        let agent_row = rows.iter().find(|r| r.role == "agent").unwrap();
        let agent_json: serde_json::Value = serde_json::from_str(&agent_row.content_json).unwrap();
        assert_eq!(agent_json["text"], "最终答案");

        // Replay rebuild keeps the paired call+output (no orphan), in order.
        let (replay, _summaries) = rebuild_replay(&pool, "u1", "s1").await.unwrap();
        let kinds: Vec<&str> = replay
            .iter()
            .map(|v| {
                v.get("type")
                    .and_then(|t| t.as_str())
                    .or_else(|| v.get("role").and_then(|r| r.as_str()))
                    .unwrap_or("?")
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["user", "function_call", "function_call_output", "assistant"]
        );
    }

    #[derive(Default)]
    struct ToolThenCompactThenAnswerProvider {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl LlmProvider for ToolThenCompactThenAnswerProvider {
        fn name(&self) -> &str {
            "tool-compact-answer"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            match *calls {
                // Iteration 0: a tool call + a Usage report above the real
                // gpt-5.5 threshold (244.8K), so the next iteration must compact.
                1 => Ok(Box::pin(stream::iter(vec![
                    Ok(LlmEvent::FunctionCall {
                        call_id: "c1".to_string(),
                        name: "get_financials".to_string(),
                        arguments: "{}".to_string(),
                    }),
                    Ok(LlmEvent::Usage(Usage {
                        input_tokens: 300_000,
                        output_tokens: 10,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    })),
                    Ok(LlmEvent::MessageEnd {
                        stop_reason: StopReason::EndTurn,
                    }),
                ]))),
                // Call 2 is the summarizer invoked by compact_session_tail.
                2 => Ok(Box::pin(stream::iter(vec![
                    Ok(LlmEvent::TextDelta {
                        text: "会话摘要".to_string(),
                    }),
                    Ok(LlmEvent::MessageEnd {
                        stop_reason: StopReason::EndTurn,
                    }),
                ]))),
                // Call 3: the main loop resumes from the summary and answers.
                _ => Ok(Box::pin(stream::iter(vec![
                    Ok(LlmEvent::TextDelta {
                        text: "压缩后的最终回答".to_string(),
                    }),
                    Ok(LlmEvent::MessageEnd {
                        stop_reason: StopReason::EndTurn,
                    }),
                ]))),
            }
        }
    }

    #[tokio::test]
    async fn mid_turn_compaction_triggers_on_token_pressure_and_continues() {
        let pool = create_agent_test_pool().await;
        vault_messages::insert(
            &pool,
            "u1",
            "s1",
            "user",
            &serde_json::json!({ "type": "text", "text": "做一次深入分析" }),
            None,
        )
        .await
        .unwrap();

        run_chat_reply(
            pool.clone(),
            "u1".to_string(),
            "s1".to_string(),
            Arc::new(ToolThenCompactThenAnswerProvider::default()),
            EventBus::new(),
            CancellationToken::new(),
            ToolRegistry::empty(),
        )
        .await
        .unwrap();

        let rows = vault_messages::list(&pool, "u1", "s1", None, 100)
            .await
            .unwrap();
        let roles: Vec<&str> = rows.iter().map(|r| r.role.as_str()).collect();
        // The long turn compacted mid-flight, then finished from the summary.
        assert_eq!(
            roles,
            vec![
                "user",
                "assistant_tool_calls",
                "tool_result",
                "compaction_summary",
                "agent"
            ]
        );

        let summary_row = rows
            .iter()
            .find(|r| r.role == "compaction_summary")
            .unwrap();
        let summary_json: serde_json::Value =
            serde_json::from_str(&summary_row.content_json).unwrap();
        assert_eq!(summary_json["text"], "会话摘要");

        let agent_row = rows.iter().find(|r| r.role == "agent").unwrap();
        let agent_json: serde_json::Value = serde_json::from_str(&agent_row.content_json).unwrap();
        assert_eq!(agent_json["text"], "压缩后的最终回答");

        // The next turn replays only the summary (tool trace is pre-boundary).
        let (replay, summaries) = rebuild_replay(&pool, "u1", "s1").await.unwrap();
        assert_eq!(summaries, vec!["会话摘要".to_string()]);
        assert_eq!(replay.last().unwrap()["content"], "压缩后的最终回答");
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
            "CREATE TABLE llm_usage_log (
                user_id TEXT NOT NULL,
                id TEXT NOT NULL,
                provider_name TEXT NOT NULL,
                model TEXT NOT NULL,
                invoker TEXT NOT NULL,
                session_id TEXT,
                task_id TEXT,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER NOT NULL,
                success INTEGER NOT NULL,
                error TEXT,
                started_at TEXT NOT NULL,
                PRIMARY KEY (user_id, id)
            )",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool
    }

    #[test]
    fn tool_batch_trace_summarizes_unique_tool_names() {
        let trace = format_tool_batch_trace(&[
            PendingCall {
                call_id: "a".into(),
                name: "get_financials".into(),
                arguments: "{}".into(),
            },
            PendingCall {
                call_id: "b".into(),
                name: "get_company_info".into(),
                arguments: "{}".into(),
            },
        ]);
        assert!(trace.contains("2 个工具"));
        assert!(trace.contains("get_company_info"));
        assert!(trace.contains("get_financials"));
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
