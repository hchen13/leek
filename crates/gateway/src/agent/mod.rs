//! The agent loop — M1.
//!
//! One turn = one user prompt → one final assistant message. Inside a turn
//! the loop runs the model–tool cycle: call the model, stream its text,
//! dispatch any tool calls, feed the results back, repeat — until the model
//! finishes, or a guard stops it (ARCHITECTURE §4.1, §5; MILESTONES M1).
//!
//! Loop discipline (harness-engineering ch.2): completion is the provider's
//! structured stop signal or a typed guard condition — never parsed prose.
//! Every guard is a *recovery* boundary (ch.8): it ends the turn with a
//! persisted, user-visible partial result and a `turn_metrics` row, never an
//! empty silent failure.

mod guards;
mod prompt;
mod tools;

pub use guards::GuardConfig;

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;

use crate::api::AppState;
use crate::llm::{pricing, ChatMessage, ChatRequest, LlmEvent, Role, StopReason};
use crate::vault::{messages, sessions, turn_metrics};

/// The model leek runs on, with its fixed M1 inference settings. There is no
/// settings surface yet, so the main-agent values are constants: XHigh
/// reasoning (the synthesizing agent gets the biggest thinking budget), Low
/// verbosity. A tuning surface is a later milestone.
const MODEL: &str = "gpt-5.5";
const REASONING_EFFORT: &str = "xhigh";
const VERBOSITY: &str = "low";

/// Recent messages of history sent as context. The auto-compaction guard
/// covers the case where even this is too large.
const HISTORY_LIMIT: i64 = 400;

/// Run one agent turn to completion. Spawned fire-and-forget by the message
/// endpoint, so it owns all of its error handling: every exit path persists
/// an assistant message, a `turn_metrics` row, and an `assistant_done` event.
pub async fn run_turn(st: AppState, session_id: String, turn_id: String) {
    if let Err(e) = drive(&st, &session_id, &turn_id).await {
        // Only the storage layer reaches here; provider / tool failures are
        // caught inside `drive` and turned into a `fatal_error` turn outcome.
        tracing::error!(error = %e, session_id, turn_id, "agent turn failed at the storage layer");
        st.emit(
            &session_id,
            "error",
            serde_json::json!({ "turn_id": turn_id, "message": e.to_string() }),
        )
        .await;
    }
}

async fn drive(st: &AppState, session_id: &str, turn_id: &str) -> Result<()> {
    let guards = st.guards;
    let started = Instant::now();
    let started_at = chrono::Utc::now();
    let wall_deadline = guards.wall_clock.map(|d| started + d);

    // ── context ─────────────────────────────────────────────────────────
    // Full session history (the just-posted user message is already in it).
    let history = messages::list(&st.pool, session_id, None, HISTORY_LIMIT).await?;
    let chat_messages: Vec<ChatMessage> = history
        .iter()
        .filter_map(|m| {
            let role = match m.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return None,
            };
            Some(ChatMessage {
                role,
                content: m.content.clone(),
            })
        })
        .collect();

    let tool_specs = tools::specs();
    let system = prompt::build_system_prompt(&tool_specs);
    let compact_trigger =
        (guards.auto_compact_threshold as f64 * pricing::context_window(MODEL) as f64) as u32;

    // ── turn state ──────────────────────────────────────────────────────
    // Prior-iteration function_call / function_call_output items, re-sent so
    // the model sees the whole multi-turn tool dialog.
    let mut additional_inputs: Vec<serde_json::Value> = Vec::new();
    let mut assistant_text = String::new();
    let mut iteration_count: usize = 0;
    let mut tool_call_count: usize = 0;
    let mut tool_error_count: usize = 0;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cost_usd: f64 = 0.0;
    let mut last_input_tokens: u32 = 0;
    // Sliding window of recent (tool, args) calls for the doom-loop detector.
    let mut doom_window: VecDeque<(String, String)> = VecDeque::new();

    // Assigned by every `break 'turn` arm — declared uninitialized so the
    // compiler rejects any future exit path that forgets to set it.
    let stop_reason: String;
    let mut first_guard: Option<&'static str> = None;
    let mut fatal_error: Option<String> = None;

    'turn: loop {
        // ── pre-iteration guards ────────────────────────────────────────
        if wall_deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
            stop_reason = "wall_clock_exceeded".into();
            first_guard.get_or_insert("wall_clock_exceeded");
            break 'turn;
        }
        if guards
            .max_iterations
            .map(|c| iteration_count >= c)
            .unwrap_or(false)
        {
            stop_reason = "max_iterations".into();
            first_guard.get_or_insert("max_iterations");
            break 'turn;
        }
        if guards.cost_cap_usd.map(|c| cost_usd >= c).unwrap_or(false) {
            stop_reason = "cost_cap_exceeded".into();
            first_guard.get_or_insert("cost_cap_exceeded");
            break 'turn;
        }
        if compact_trigger > 0 && last_input_tokens >= compact_trigger {
            // M1 auto-compaction: detect the 90% threshold and stop the turn
            // with a clear diagnostic. Summarize-and-continue compaction is a
            // later milestone (see docs / the M1 report).
            stop_reason = "context_limit".into();
            first_guard.get_or_insert("context_limit");
            break 'turn;
        }

        iteration_count += 1;

        // ── per-iteration developer hint: wall-clock soft prompt ────────
        let mut iter_inputs = additional_inputs.clone();
        if let Some(deadline) = wall_deadline {
            let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
            if let Some(hint) = guards::soft_deadline_hint(remaining) {
                iter_inputs.push(serde_json::json!({
                    "role": "developer",
                    "content": format!("[本回合剩余约 {remaining} 秒] {hint}"),
                }));
            }
        }

        let req = ChatRequest {
            model: MODEL.to_string(),
            system: system.clone(),
            messages: chat_messages.clone(),
            tools: tool_specs.clone(),
            additional_inputs: iter_inputs,
            reasoning_effort: Some(REASONING_EFFORT.to_string()),
            verbosity: Some(VERBOSITY.to_string()),
        };

        // ── call the model ──────────────────────────────────────────────
        let mut stream = match st.codex.chat(req).await {
            Ok(s) => s,
            Err(e) => {
                fatal_error = Some(e.to_string());
                stop_reason = "fatal_error".into();
                break 'turn;
            }
        };

        // ── consume the streamed response (idle-timeout guarded) ────────
        let mut pending: Vec<(String, String, String)> = Vec::new();
        let mut iter_stop: Option<StopReason> = None;
        let mut idle_hit = false;
        'stream: loop {
            // The idle timer wraps every `next()`: a `Ping` (reasoning
            // lifecycle event) resets it, so only genuine silence trips it.
            let item = match guards.idle_timeout {
                Some(d) => match tokio::time::timeout(d, stream.next()).await {
                    Ok(item) => item,
                    Err(_) => {
                        idle_hit = true;
                        break 'stream;
                    }
                },
                None => stream.next().await,
            };
            let Some(event) = item else { break 'stream };
            match event {
                Ok(LlmEvent::TextDelta { text }) => {
                    assistant_text.push_str(&text);
                    st.emit_ephemeral(
                        session_id,
                        "assistant_delta",
                        serde_json::json!({ "turn_id": turn_id, "text": text }),
                    )
                    .await;
                }
                Ok(LlmEvent::FunctionCall {
                    call_id,
                    name,
                    arguments,
                }) => pending.push((call_id, name, arguments)),
                Ok(LlmEvent::Usage(u)) => {
                    input_tokens += u.input_tokens as u64;
                    output_tokens += u.output_tokens as u64;
                    cost_usd += pricing::compute_cost(MODEL, &u);
                    last_input_tokens = u.input_tokens;
                }
                Ok(LlmEvent::MessageEnd { stop_reason: sr }) => iter_stop = Some(sr),
                Ok(LlmEvent::Ping) => {}
                Err(e) => {
                    fatal_error = Some(e.to_string());
                    break 'stream;
                }
            }
        }

        if fatal_error.is_some() {
            stop_reason = "fatal_error".into();
            break 'turn;
        }
        if idle_hit {
            stop_reason = "idle_timeout".into();
            first_guard.get_or_insert("idle_timeout");
            break 'turn;
        }

        // ── no tool calls → the model finished ──────────────────────────
        if pending.is_empty() {
            stop_reason = match iter_stop {
                Some(StopReason::MaxTokens) => "max_tokens".into(),
                _ => "end_turn".into(),
            };
            break 'turn;
        }

        // ── dispatch tool calls, re-inject results, loop ────────────────
        let mut doom_hit = false;
        for (call_id, name, arguments) in pending {
            tool_call_count += 1;

            if let Some(threshold) = guards.doom_loop_threshold {
                doom_window.push_back((name.clone(), arguments.clone()));
                while doom_window.len() > threshold {
                    doom_window.pop_front();
                }
                if guards::detect_doom_loop(&doom_window, threshold) {
                    doom_hit = true;
                }
            }

            st.emit(
                session_id,
                "tool_call",
                serde_json::json!({
                    "turn_id": turn_id, "call_id": call_id,
                    "name": name, "arguments": arguments,
                }),
            )
            .await;

            let args_value: serde_json::Value =
                serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null);
            let outcome = tools::dispatch(&st.http, &name, &args_value).await;
            if outcome.is_error {
                tool_error_count += 1;
            }

            st.emit(
                session_id,
                "tool_result",
                serde_json::json!({
                    "turn_id": turn_id, "call_id": call_id, "name": name,
                    "is_error": outcome.is_error,
                    "output_preview": preview(&outcome.output),
                }),
            )
            .await;

            // Re-inject the call and its result so the next iteration sees
            // the full tool dialog (order matters: call before output).
            additional_inputs.push(serde_json::json!({
                "type": "function_call", "call_id": call_id,
                "name": name, "arguments": arguments,
            }));
            additional_inputs.push(serde_json::json!({
                "type": "function_call_output", "call_id": call_id,
                "output": outcome.output,
            }));
        }

        if doom_hit {
            stop_reason = "doom_loop".into();
            first_guard.get_or_insert("doom_loop");
            break 'turn;
        }
    }

    // ── finalize: assistant message + metrics + lifecycle events ────────
    let final_text = compose_final_text(&assistant_text, &stop_reason, fatal_error.as_deref());
    let assistant = messages::insert(&st.pool, session_id, "assistant", &final_text).await?;
    st.emit(
        session_id,
        "message_created",
        serde_json::json!({
            "seq": assistant.seq, "role": "assistant",
            "content": assistant.content, "created_at": assistant.created_at,
        }),
    )
    .await;

    let ended_at = chrono::Utc::now();
    let wall_ms = started.elapsed().as_millis() as i64;
    turn_metrics::insert(
        &st.pool,
        &turn_metrics::NewTurnMetrics {
            turn_id,
            session_id,
            model: MODEL,
            started_at: &started_at.to_rfc3339(),
            ended_at: &ended_at.to_rfc3339(),
            wall_clock_ms: wall_ms,
            iteration_count: iteration_count as i64,
            tool_call_count: tool_call_count as i64,
            tool_error_count: tool_error_count as i64,
            input_tokens: input_tokens as i64,
            output_tokens: output_tokens as i64,
            cost_usd,
            stop_reason: &stop_reason,
            first_triggered_guard: first_guard,
            fatal_error: fatal_error.as_deref(),
        },
    )
    .await?;

    st.emit(
        session_id,
        "turn_metrics_recorded",
        serde_json::json!({
            "turn_id": turn_id,
            "stop_reason": stop_reason,
            "first_triggered_guard": first_guard,
            "iteration_count": iteration_count,
            "tool_call_count": tool_call_count,
            "tool_error_count": tool_error_count,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cost_usd": cost_usd,
            "wall_clock_ms": wall_ms,
            "model": MODEL,
        }),
    )
    .await;

    st.emit(
        session_id,
        "assistant_done",
        serde_json::json!({
            "turn_id": turn_id,
            "message_seq": assistant.seq,
            "stop_reason": stop_reason,
        }),
    )
    .await;

    sessions::touch(&st.pool, session_id).await?;
    tracing::info!(session_id, turn_id, %stop_reason, iteration_count, "agent turn complete");
    Ok(())
}

/// Short preview of a tool output for the SSE `tool_result` event — the model
/// gets the full output via `function_call_output`; the event is just for UI.
fn preview(s: &str) -> String {
    const MAX: usize = 280;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}

/// Compose the persisted assistant message. A guard- or error-stopped turn
/// still produces a user-visible message — the partial text (if any) plus an
/// honest note about why it stopped (harness-engineering ch.8: no silent
/// empty result).
fn compose_final_text(text: &str, stop_reason: &str, fatal: Option<&str>) -> String {
    let body = text.trim();
    if stop_reason == "fatal_error" {
        let err = fatal.unwrap_or("未知错误");
        return if body.is_empty() {
            format!("本回合调用失败：{err}")
        } else {
            format!("{body}\n\n[本回合中断：{err}]")
        };
    }
    match (body.is_empty(), stop_note(stop_reason)) {
        (true, Some(note)) => format!("[{note}]"),
        (true, None) => "（本回合模型没有输出文本。）".to_string(),
        (false, Some(note)) => format!("{body}\n\n[{note}]"),
        (false, None) => body.to_string(),
    }
}

/// Human-readable note for a non-natural stop. `None` for `end_turn`.
fn stop_note(stop_reason: &str) -> Option<&'static str> {
    match stop_reason {
        "end_turn" => None,
        "max_tokens" => Some("模型输出达到长度上限，可能未写完。"),
        "idle_timeout" => Some("模型响应空闲超时（idle timeout），本回合提前结束。"),
        "wall_clock_exceeded" => Some("达到本回合时间上限（wall-clock），提前结束。"),
        "max_iterations" => Some("达到迭代次数上限（iteration cap），提前结束。"),
        "cost_cap_exceeded" => Some("达到本回合成本上限（cost cap），提前结束。"),
        "doom_loop" => Some("检测到工具调用陷入循环（doom-loop），本回合中止。"),
        "context_limit" => {
            Some("上下文接近窗口上限（auto-compaction 阈值），本回合提前结束；建议精简会话或新开 session。")
        }
        _ => Some("本回合提前结束。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_text_plain_on_natural_end() {
        assert_eq!(compose_final_text("答案。", "end_turn", None), "答案。");
    }

    #[test]
    fn final_text_appends_guard_note() {
        let out = compose_final_text("部分分析…", "wall_clock_exceeded", None);
        assert!(out.starts_with("部分分析…"));
        assert!(out.contains("wall-clock"));
    }

    #[test]
    fn final_text_is_never_empty_on_guard_stop() {
        let out = compose_final_text("", "idle_timeout", None);
        assert!(!out.is_empty());
        assert!(out.contains("idle timeout"));
    }

    #[test]
    fn final_text_reports_fatal_error() {
        let out = compose_final_text("", "fatal_error", Some("HTTP 401"));
        assert!(out.contains("HTTP 401"));
    }

    #[test]
    fn preview_truncates_long_output() {
        let long = "x".repeat(1000);
        let p = preview(&long);
        assert!(p.chars().count() <= 281);
        assert!(p.ends_with('…'));
    }
}
