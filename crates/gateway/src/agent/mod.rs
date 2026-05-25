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
//!
//! Auto-compaction is the one boundary that recovers by *continuing*: near
//! the context-window limit it folds the early context into a summary and
//! runs on, instead of stopping (see `compaction`; REQUIREMENTS §7.1).
//!
//! The loop emits the M1.9 workbench event contract (see `events`): every
//! event names the surface that consumes it, and the canvas process
//! artifacts — note trace, tool lifecycle, provider search — share one
//! envelope.

mod builtin_governance;
mod compaction;
#[cfg(test)]
mod compaction_replay_test;
pub mod events;
pub mod fatal;
mod guards;
mod loop_core;
mod prompt;
mod subagent;
mod tools;

pub use guards::GuardConfig;

use anyhow::Result;

use crate::api::AppState;
use crate::hooks::{HookEvent, HookOutcome};
use crate::llm::{ChatMessage, Role, WebSearchAction};
use crate::vault::{messages, sessions, turn_metrics};

/// The model leek runs on, with its fixed M1 inference settings. There is no
/// settings surface yet, so the main-agent values are constants: XHigh
/// reasoning (the synthesizing agent gets the biggest thinking budget), Low
/// verbosity. A tuning surface is a later milestone.
pub(super) const MODEL: &str = "gpt-5.5";
pub(super) const REASONING_EFFORT: &str = "xhigh";
pub(super) const VERBOSITY: &str = "low";

/// Cap on session history loaded as a turn's starting context. A turn whose
/// context still outgrows the model window is handled by auto-compaction
/// (see `compaction`), which folds the early context and continues.
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
            events::kind::ERROR,
            serde_json::json!({ "turn_id": turn_id, "message": e.to_string() }),
        )
        .await;
    }
}

async fn drive(st: &AppState, session_id: &str, turn_id: &str) -> Result<()> {
    // Snapshot the live settings once per turn — a mid-turn PATCH only
    // affects the *next* turn, so the guard set stays stable for the
    // duration of this loop (M2.6).
    let guards = guards::GuardConfig::resolve(&st.config_snapshot());

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

    // ── lifecycle hooks: SessionStart (first turn only) + UserPromptSubmit ─
    // SessionStart fires once per session — we detect "first turn" by the
    // user message in history being the only message (no assistants yet).
    let assistant_count = history.iter().filter(|m| m.role == "assistant").count();
    if assistant_count == 0 && st.hooks.has_event(HookEvent::SessionStart) {
        let _ = st
            .hooks
            .trigger(
                HookEvent::SessionStart,
                "startup",
                session_start_payload(session_id),
            )
            .await;
    }
    if st.hooks.has_event(HookEvent::UserPromptSubmit) {
        let latest_user_msg = history.iter().rev().find(|m| m.role == "user");
        let prompt_text = latest_user_msg.map(|m| m.content.as_str()).unwrap_or("");
        let outcome = st
            .hooks
            .trigger(
                HookEvent::UserPromptSubmit,
                "",
                user_prompt_submit_payload(session_id, turn_id, prompt_text),
            )
            .await;
        if let HookOutcome::Block { reason } = outcome {
            // Block the whole turn: don't call the model at all.
            let stop_reason = "blocked_by_hook";
            let final_text = format!("[本回合被 UserPromptSubmit hook 拦截：{reason}]");
            let assistant =
                messages::insert(&st.pool, session_id, "assistant", &final_text).await?;
            st.emit(
                session_id,
                events::kind::MESSAGE_CREATED,
                serde_json::json!({
                    "seq": assistant.seq, "role": "assistant",
                    "content": assistant.content, "created_at": assistant.created_at,
                }),
            )
            .await;
            st.emit(
                session_id,
                events::kind::ASSISTANT_DONE,
                serde_json::json!({
                    "turn_id": turn_id,
                    "message_seq": assistant.seq,
                    "stop_reason": stop_reason,
                }),
            )
            .await;
            tracing::info!(
                session_id,
                turn_id,
                reason,
                "user prompt blocked by UserPromptSubmit hook"
            );
            return Ok(());
        }
    }

    let tool_specs = tools::specs(&st.skills, &st.agents);
    let system = prompt::build_system_prompt(&tool_specs, &st.skills, &st.agents);

    // ── M3.2: register a user-abort signal slot for this turn. The slot
    // lives in `AppState::abort_signals`; the `POST .../abort` endpoint
    // looks it up by `turn_id` and fires `notify_one()`. We hold the
    // `Arc<Notify>` ourselves (an `_AbortGuard` removes the slot on
    // drop even if the loop panics — leaking would let a future endpoint
    // call hit a notify nobody is listening on, plus accumulate slots
    // forever for long-lived processes).
    let abort_signal = st.register_abort_signal(turn_id);
    let _abort_guard = AbortGuard {
        st,
        turn_id: turn_id.to_string(),
    };

    // ── inner loop (shared with subagents — M2.7) ───────────────────────
    // The main-agent loop has nothing special inside the model→tool cycle;
    // it shares one implementation with every subagent loop. The wrapping
    // around the loop is the only thing that differs (lifecycle hooks,
    // message persistence, the right-rail / chat surfaces).
    let outcome = loop_core::run_loop(loop_core::LoopParams {
        st,
        session_id,
        turn_id,
        parent_turn_id: None,
        depth: 0,
        system,
        messages: chat_messages,
        tool_specs,
        allowed_tools: loop_core::ToolAllowlist::All,
        guards,
        abort_signal: Some(abort_signal),
        // Main agent inherits the env-resolved capability flag.
        web_search: st.web_search,
    })
    .await?;

    // ── finalize: assistant message + metrics + lifecycle events ────────
    // M3.3: hand the typed FatalReason to the composer so the persisted
    // message can use the kind-specific hint ("稍后重试" / "codex 静默 N 秒"
    // / etc.) instead of the bare anyhow string.
    let final_text = compose_final_text(
        &outcome.final_reply,
        &outcome.stop_reason,
        outcome.fatal_error.as_deref(),
        outcome.fatal_reason.as_ref(),
    );
    let assistant = messages::insert(&st.pool, session_id, "assistant", &final_text).await?;
    st.emit(
        session_id,
        events::kind::MESSAGE_CREATED,
        serde_json::json!({
            "seq": assistant.seq, "role": "assistant",
            "content": assistant.content, "created_at": assistant.created_at,
        }),
    )
    .await;

    // M3.3: split the typed fatal_reason into its column-shaped pieces so the
    // borrow checker is happy across the insert call.
    let fatal_reason_kind = outcome.fatal_reason.as_ref().map(|r| r.kind().to_string());
    let fatal_reason_detail = outcome.fatal_reason.as_ref().map(|r| r.detail());

    turn_metrics::insert(
        &st.pool,
        &turn_metrics::NewTurnMetrics {
            turn_id,
            session_id,
            model: MODEL,
            started_at: &outcome.started_at.to_rfc3339(),
            ended_at: &outcome.ended_at.to_rfc3339(),
            wall_clock_ms: outcome.wall_clock_ms,
            iteration_count: outcome.iteration_count as i64,
            tool_call_count: outcome.tool_call_count as i64,
            tool_error_count: outcome.tool_error_count as i64,
            compaction_count: outcome.compaction_count as i64,
            input_tokens: outcome.input_tokens as i64,
            output_tokens: outcome.output_tokens as i64,
            cost_usd: outcome.cost_usd,
            stop_reason: &outcome.stop_reason,
            first_triggered_guard: outcome.first_guard,
            fatal_error: outcome.fatal_error.as_deref(),
            fatal_reason_kind: fatal_reason_kind.as_deref(),
            fatal_reason_detail: fatal_reason_detail.as_deref(),
            // The main agent is the root of the turn tree — no parent,
            // depth 0. M2.7 added these fields; subagents fill them in.
            parent_turn_id: None,
            depth: 0,
        },
    )
    .await?;

    st.emit(
        session_id,
        events::kind::TURN_METRICS_RECORDED,
        serde_json::json!({
            "turn_id": turn_id,
            "stop_reason": outcome.stop_reason,
            "first_triggered_guard": outcome.first_guard,
            "iteration_count": outcome.iteration_count,
            "tool_call_count": outcome.tool_call_count,
            "tool_error_count": outcome.tool_error_count,
            "compaction_count": outcome.compaction_count,
            "input_tokens": outcome.input_tokens,
            "output_tokens": outcome.output_tokens,
            "cost_usd": outcome.cost_usd,
            "wall_clock_ms": outcome.wall_clock_ms,
            "model": MODEL,
        }),
    )
    .await;

    // M3.3: include the structured `fatal_reason` payload so Chat.tsx can
    // render an actionable hint card (kind / detail / hint) instead of the
    // generic "本回合调用失败：POST to the codex Responses endpoint" string.
    // Populated on every fatal_error stop AND on idle_timeout stops (the
    // substantive-only idle detector trips with a CodexStreamSilent reason).
    let mut done_payload = serde_json::json!({
        "turn_id": turn_id,
        "message_seq": assistant.seq,
        "stop_reason": outcome.stop_reason,
    });
    if let Some(reason) = outcome.fatal_reason.as_ref() {
        done_payload["fatal_reason"] = reason.to_payload();
    }
    st.emit(session_id, events::kind::ASSISTANT_DONE, done_payload)
        .await;

    // ── Stop hook (M2.5) ────────────────────────────────────────────────
    // Advisory: turn already ended, so a block verdict only logs. The
    // payload mirrors CC's `Stop` shape — `response` is the final text
    // the user sees.
    if st.hooks.has_event(HookEvent::Stop) {
        let _ = st
            .hooks
            .trigger(
                HookEvent::Stop,
                "",
                stop_payload(session_id, turn_id, &outcome.stop_reason, &final_text),
            )
            .await;
    }

    sessions::touch(&st.pool, session_id).await?;
    tracing::info!(
        session_id,
        turn_id,
        stop_reason = %outcome.stop_reason,
        iteration_count = outcome.iteration_count,
        "agent turn complete"
    );
    Ok(())
}

/// Short, char-safe preview of a long string for an SSE event payload —
/// used by auto-compaction for its `summary_preview`.
fn preview(s: &str) -> String {
    const MAX: usize = 280;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}

/// Build the `plan_updated` payload from `update_plan`'s display payload.
/// The loop forwards the tool's structured plan to the right-rail widget
/// (REQUIREMENTS §2.6) verbatim — it does not interpret the plan.
pub(super) fn plan_payload(turn_id: &str, outcome: &tools::ToolOutcome) -> serde_json::Value {
    let display = &outcome.display_payload;
    serde_json::json!({
        "turn_id": turn_id,
        "plan": display
            .get("plan")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        "explanation": display
            .get("explanation")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    })
}

/// Host of a provider-search URL, for the canvas search card. A URL that
/// does not parse is carried without a host (the card shows the URL).
fn web_search_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// Map a `WebSearchAction` variant to the `search_lifecycle` `data` body the
/// frontend renders. `None` (the `Start` frame) yields an empty object —
/// the activity is only known on completion. Each variant tags its body
/// with `action_type` so the renderer never guesses the activity from the
/// presence or absence of fields.
pub(super) fn build_search_data(action: Option<&WebSearchAction>) -> serde_json::Value {
    match action {
        None => serde_json::json!({}),
        Some(WebSearchAction::Search { query, results }) => {
            let results_json: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "title": r.title,
                        "url": r.url,
                        "host": web_search_host(&r.url),
                    })
                })
                .collect();
            let total = results_json.len();
            serde_json::json!({
                "action_type": "search",
                "query": query,
                "results": results_json,
                "results_total": total,
            })
        }
        Some(WebSearchAction::OpenPage { url, title, snippet }) => {
            serde_json::json!({
                "action_type": "open_page",
                "url": url,
                "host": web_search_host(url),
                "title": title,
                "snippet": snippet,
            })
        }
        Some(WebSearchAction::FindInPage { url, pattern, matches }) => {
            serde_json::json!({
                "action_type": "find_in_page",
                "url": url,
                "host": web_search_host(url),
                "pattern": pattern,
                "matches": matches,
            })
        }
        Some(WebSearchAction::Unknown { kind }) => {
            serde_json::json!({ "action_type": kind })
        }
    }
}

/// Compose the persisted assistant message. A guard- or error-stopped turn
/// still produces a user-visible message — the partial text (if any) plus an
/// honest note about why it stopped (harness-engineering ch.8: no silent
/// empty result).
///
/// M3.3: when a typed `FatalReason` is available, the persisted message uses
/// its hint (actionable, kind-specific) instead of the raw fatal-error string.
/// The chat hint card on the frontend is the primary surface; this string is
/// the fallback (history pages, search index, terminal grep).
fn compose_final_text(
    text: &str,
    stop_reason: &str,
    fatal: Option<&str>,
    reason: Option<&fatal::FatalReason>,
) -> String {
    let body = text.trim();
    if stop_reason == "fatal_error" {
        // Prefer the typed reason — its hint is user-friendly Chinese
        // ("稍后重试"), the raw fatal is the anyhow chain ("HTTP 503: …").
        let err: String = reason
            .map(|r| r.hint())
            .unwrap_or_else(|| fatal.unwrap_or("未知错误").to_string());
        return if body.is_empty() {
            format!("本回合调用失败：{err}")
        } else {
            format!("{body}\n\n[本回合中断：{err}]")
        };
    }
    // For non-fatal stops with a typed reason (today: idle_timeout →
    // CodexStreamSilent), prefer the reason's hint over the static
    // stop_note — it carries the silent_secs number so the user can see
    // exactly which threshold tripped.
    let note: Option<String> = reason
        .filter(|_| stop_reason == "idle_timeout")
        .map(|r| r.hint())
        .or_else(|| stop_note(stop_reason).map(str::to_string));
    match (body.is_empty(), note) {
        (true, Some(n)) => format!("[{n}]"),
        (true, None) => "（本回合模型没有输出文本。）".to_string(),
        (false, Some(n)) => format!("{body}\n\n[{n}]"),
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
        "codex_duplicate_abort" => {
            Some("检测到 codex 内置 web_search 重复 open 同一 URL，本回合中止。")
        }
        // M3.2: user clicked 强制停止本回合. Honest, short — the abort
        // button is right there in the UI so the user already knows
        // they did it; this note exists so the persisted assistant
        // message reads as something other than an empty bubble.
        "user_aborted" => Some("本回合已被你手动停止。"),
        _ => Some("本回合提前结束。"),
    }
}

/// M3.2: drop guard that removes a turn's abort-signal slot when the
/// agent loop exits — including panic / early-return paths. Without
/// this, a panic in `drive` after registration would leak the slot
/// forever, and a stale endpoint call could fire a notify on an
/// `Arc<Notify>` no future is listening on (harmless, but the slot
/// itself never gets reclaimed).
struct AbortGuard<'a> {
    st: &'a crate::api::AppState,
    turn_id: String,
}

impl<'a> Drop for AbortGuard<'a> {
    fn drop(&mut self) {
        self.st.remove_abort_signal(&self.turn_id);
    }
}

// ── Hook payload builders ──────────────────────────────────────────────
// Each event's payload mirrors the CC `hook_event_name` schema (M2.5
// research notes §4). leek doesn't carry CC's `transcript_path` /
// `cwd` / `permission_mode` fields — they're empty stubs for shape
// parity, so hooks ported from a CC config can read them safely.

fn session_start_payload(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "hook_event_name": "SessionStart",
        "source": "startup",
    })
}

fn user_prompt_submit_payload(
    session_id: &str,
    turn_id: &str,
    prompt: &str,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    })
}

pub(super) fn pre_tool_use_payload(
    session_id: &str,
    turn_id: &str,
    iteration: usize,
    call_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "iteration": iteration,
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_use_id": call_id,
    })
}

pub(super) fn post_tool_use_payload(
    session_id: &str,
    turn_id: &str,
    iteration: usize,
    call_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    outcome: &tools::ToolOutcome,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "iteration": iteration,
        "hook_event_name": "PostToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
        "tool_result": outcome.model_output,
        "is_error": outcome.is_error,
        "tool_use_id": call_id,
    })
}

pub(super) fn pre_compact_payload(session_id: &str, turn_id: &str, tokens_before: u32) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "hook_event_name": "PreCompact",
        "compaction_trigger": "auto",
        "tokens_before": tokens_before,
    })
}

fn stop_payload(
    session_id: &str,
    turn_id: &str,
    stop_reason: &str,
    response: &str,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "hook_event_name": "Stop",
        "stop_reason": stop_reason,
        "response": response,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_text_plain_on_natural_end() {
        assert_eq!(
            compose_final_text("答案。", "end_turn", None, None),
            "答案。"
        );
    }

    #[test]
    fn final_text_appends_guard_note() {
        let out = compose_final_text("部分分析…", "wall_clock_exceeded", None, None);
        assert!(out.starts_with("部分分析…"));
        assert!(out.contains("wall-clock"));
    }

    #[test]
    fn final_text_is_never_empty_on_guard_stop() {
        let out = compose_final_text("", "idle_timeout", None, None);
        assert!(!out.is_empty());
        assert!(out.contains("idle timeout"));
    }

    #[test]
    fn final_text_reports_fatal_error() {
        let out = compose_final_text("", "fatal_error", Some("HTTP 401"), None);
        assert!(out.contains("HTTP 401"));
    }

    #[test]
    fn final_text_uses_fatal_reason_hint_when_present() {
        // M3.3: when a typed FatalReason is available, the persisted
        // assistant message uses its actionable hint ("稍后重试") instead
        // of the raw anyhow string.
        let reason = fatal::FatalReason::CodexHttp5xx {
            status: 503,
            body_excerpt: "service unavailable".into(),
            retry_attempts: 1,
        };
        let out = compose_final_text("", "fatal_error", Some("anyhow chain"), Some(&reason));
        assert!(out.contains("稍后重试"));
        // The hint takes precedence over the raw `fatal` string
        assert!(!out.contains("anyhow chain"));
    }

    #[test]
    fn final_text_idle_timeout_uses_silent_hint_when_present() {
        // M3.3: idle_timeout + CodexStreamSilent reason → the persisted
        // message carries the silent_secs number so the user can see
        // exactly which threshold tripped.
        let reason = fatal::FatalReason::CodexStreamSilent {
            silent_secs: 90,
            retry_attempts: 1,
        };
        let out = compose_final_text("", "idle_timeout", None, Some(&reason));
        assert!(out.contains("90"));
        assert!(out.contains("静默"));
    }

    #[test]
    fn preview_truncates_long_output() {
        let long = "x".repeat(1000);
        let p = preview(&long);
        assert!(p.chars().count() <= 281);
        assert!(p.ends_with('…'));
    }
}
