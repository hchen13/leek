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

mod compaction;
#[cfg(test)]
mod compaction_replay_test;
pub mod events;
mod guards;
mod prompt;
mod tools;

pub use guards::GuardConfig;

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;

use crate::api::AppState;
use crate::hooks::{HookEvent, HookOutcome};
use crate::llm::{
    pricing, ChatMessage, ChatRequest, LlmEvent, Role, StopReason, WebSearchAction,
    WebSearchPhase,
};
use crate::vault::{messages, sessions, turn_metrics};

/// The model leek runs on, with its fixed M1 inference settings. There is no
/// settings surface yet, so the main-agent values are constants: XHigh
/// reasoning (the synthesizing agent gets the biggest thinking budget), Low
/// verbosity. A tuning surface is a later milestone.
const MODEL: &str = "gpt-5.5";
const REASONING_EFFORT: &str = "xhigh";
const VERBOSITY: &str = "low";

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
    let started = Instant::now();
    let started_at = chrono::Utc::now();
    let wall_deadline = guards.wall_clock.map(|d| started + d);

    // ── context ─────────────────────────────────────────────────────────
    // Full session history (the just-posted user message is already in it).
    let history = messages::list(&st.pool, session_id, None, HISTORY_LIMIT).await?;
    let mut chat_messages: Vec<ChatMessage> = history
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

    let tool_specs = tools::specs(&st.skills);
    let system = prompt::build_system_prompt(&tool_specs, &st.skills);
    // Auto-compaction trigger: a fraction of the context window. The window
    // is `LEEK_CONTEXT_WINDOW` if set (a small value trips compaction within
    // a few turns — handy for tests), else the per-model `pricing` value.
    let context_window = guards
        .context_window
        .unwrap_or_else(|| pricing::context_window(MODEL));
    let compact_trigger = (guards.auto_compact_threshold as f64 * context_window as f64) as u32;

    // ── turn state ──────────────────────────────────────────────────────
    // Prior-iteration function_call / function_call_output items, re-sent so
    // the model sees the whole multi-turn tool dialog.
    let mut additional_inputs: Vec<serde_json::Value> = Vec::new();
    // The final reply only — the text of the turn-ending iteration. Text
    // from earlier iterations precedes a tool call, so it is Note Trace and
    // goes to the canvas, never into the chat message (REQUIREMENTS §2.3).
    let mut final_reply = String::new();
    let mut iteration_count: usize = 0;
    // LLM-call sequence number for the transcript archive (F2). It
    // includes BOTH main iterations and any auto-compaction summary calls
    // — every code path that hits `codex.chat` bumps this — so it
    // intentionally diverges from `iteration_count`. One row per provider
    // call lands in `llm_transcripts`.
    let mut transcript_iter: i64 = 0;
    let mut tool_call_count: usize = 0;
    let mut tool_error_count: usize = 0;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cost_usd: f64 = 0.0;
    // Sliding window of recent (tool, args) calls for the doom-loop detector.
    let mut doom_window: VecDeque<(String, String)> = VecDeque::new();
    // Auto-compaction state: the running summary of context folded away,
    // and how many times that has happened this turn.
    let mut compaction_summary: Option<String> = None;
    let mut compaction_count: usize = 0;

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
            // M2.6: emit a dedicated event ahead of the stop_reason so the
            // frontend can render a "本回合达到预算上限" warning bar without
            // having to inspect `assistant_done.stop_reason`. The
            // `assistant_done` event below still carries the same reason —
            // this one just lets the chat surface react earlier.
            let cap = guards.cost_cap_usd.unwrap_or(0.0);
            st.emit(
                session_id,
                events::kind::TURN_COST_CAPPED,
                serde_json::json!({
                    "turn_id": turn_id,
                    "cap_usd": cap,
                    "actual_cost_usd": cost_usd,
                    "iter_count": iteration_count,
                }),
            )
            .await;
            stop_reason = "cost_cap_exceeded".into();
            first_guard.get_or_insert("cost_cap_exceeded");
            break 'turn;
        }
        // ── auto-compaction ─────────────────────────────────────────────
        // Near the context-window limit, fold the early context into a
        // traceable summary and continue this turn — there is no
        // context-limit stop (REQUIREMENTS §7.1, MILESTONES M1.8). The
        // trigger signal is leek's own estimator over the assembled
        // request: codex's reported `input_tokens` mixes in builtin
        // web_search volume that compaction cannot shrink, so it cannot
        // be the trigger (M2.5 — see `docs/dispatches/M2.5-compaction-fix.md`).
        if compact_trigger > 0 {
            let ctx_tokens = compaction::estimate_context_tokens(
                &system,
                compaction_summary.as_deref(),
                &chat_messages,
                &additional_inputs,
                &tool_specs,
            );
            if ctx_tokens >= compact_trigger {
                // ── PreCompact hook (M2.5) ──────────────────────────────
                // Advisory: leek logs a block verdict but does not abort
                // compaction (compaction is a recovery boundary —
                // skipping it would silently drop tokens).
                if st.hooks.has_event(HookEvent::PreCompact) {
                    let _ = st
                        .hooks
                        .trigger(
                            HookEvent::PreCompact,
                            "auto",
                            pre_compact_payload(session_id, turn_id, ctx_tokens),
                        )
                        .await;
                }
                // Compaction's summary call is one LLM call — give it its
                // own slot in the transcript series.
                transcript_iter += 1;
                match compaction::compact(
                    st,
                    session_id,
                    turn_id,
                    &system,
                    &tool_specs,
                    compaction::TurnContext {
                        summary: &mut compaction_summary,
                        messages: &mut chat_messages,
                        tool_dialog: &mut additional_inputs,
                    },
                    ctx_tokens,
                    transcript_iter,
                    guards.idle_timeout,
                )
                .await
                {
                    Ok(compaction::Compacted::Done) => {
                        compaction_count += 1;
                    }
                    Ok(compaction::Compacted::Skipped) => {}
                    Err(e) => {
                        fatal_error = Some(e.to_string());
                        stop_reason = "fatal_error".into();
                        break 'turn;
                    }
                }
            }
        }

        iteration_count += 1;
        transcript_iter += 1;

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

        // Prepend the auto-compaction summary (if any) as a developer-role
        // context block, ahead of the kept recent messages.
        let mut iter_messages: Vec<ChatMessage> = Vec::with_capacity(chat_messages.len() + 1);
        if let Some(summary) = &compaction_summary {
            iter_messages.push(compaction::summary_message(summary));
        }
        iter_messages.extend(chat_messages.iter().cloned());

        let req = ChatRequest {
            model: MODEL.to_string(),
            system: system.clone(),
            messages: iter_messages,
            tools: tool_specs.clone(),
            additional_inputs: iter_inputs,
            reasoning_effort: Some(REASONING_EFFORT.to_string()),
            verbosity: Some(VERBOSITY.to_string()),
            web_search: st.web_search,
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            iteration: transcript_iter,
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
        // `iter_text` is this iteration's assistant text. Whether it is a
        // Note Trace or the final reply is known only once the stream ends
        // — did the model also emit tool calls? — so it is classified below.
        let mut iter_text = String::new();
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
                    iter_text.push_str(&text);
                    st.emit_ephemeral(
                        session_id,
                        events::kind::ASSISTANT_DELTA,
                        serde_json::json!({
                            "turn_id": turn_id,
                            "iteration": iteration_count,
                            "text": text,
                        }),
                    )
                    .await;
                }
                Ok(LlmEvent::FunctionCall {
                    call_id,
                    name,
                    arguments,
                }) => pending.push((call_id, name, arguments)),
                Ok(LlmEvent::WebSearch {
                    call_id,
                    phase,
                    action,
                }) => {
                    // Provider-side search (M1.9.4): observed, not
                    // dispatched — normalized to a canvas search artifact.
                    // The backend reports per-call results on the
                    // `Completed` frame via the request's
                    // `include: ["web_search_call.results"]` opt-in
                    // (MILESTONES decision 2026-05-20). The activity kind
                    // (`search` / `open_page` / `find_in_page` / unknown)
                    // is mapped to a variant-specific `data` body — the
                    // event kind stays `search_lifecycle` so the contract
                    // is stable.
                    let canvas_phase = match phase {
                        WebSearchPhase::Started => events::Phase::Start,
                        WebSearchPhase::Completed => events::Phase::Completion,
                    };
                    let data = build_search_data(action.as_ref());
                    st.emit(
                        session_id,
                        events::kind::SEARCH_LIFECYCLE,
                        events::CanvasArtifact::search(
                            turn_id,
                            iteration_count,
                            &call_id,
                            canvas_phase,
                            data,
                        )
                        .into_payload(),
                    )
                    .await;
                }
                Ok(LlmEvent::Usage(u)) => {
                    input_tokens += u.input_tokens as u64;
                    output_tokens += u.output_tokens as u64;
                    cost_usd += pricing::compute_cost(MODEL, &u);
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
            // The turn ends here — the partial text is the final reply.
            final_reply = iter_text;
            stop_reason = "fatal_error".into();
            break 'turn;
        }
        if idle_hit {
            final_reply = iter_text;
            stop_reason = "idle_timeout".into();
            first_guard.get_or_insert("idle_timeout");
            break 'turn;
        }

        // ── no tool calls → the model finished: this text is the reply ──
        if pending.is_empty() {
            final_reply = iter_text;
            stop_reason = match iter_stop {
                Some(StopReason::MaxTokens) => "max_tokens".into(),
                _ => "end_turn".into(),
            };
            break 'turn;
        }

        // The iteration also emitted tool calls, so its text is narration
        // around them — a Note Trace, shown on the canvas, never in the chat
        // message (REQUIREMENTS §2.3).
        let note = iter_text.trim();
        if !note.is_empty() {
            st.emit(
                session_id,
                events::kind::NOTE_TRACE,
                events::CanvasArtifact::note(turn_id, iteration_count, note).into_payload(),
            )
            .await;
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

            let args_value: serde_json::Value =
                serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null);

            // Where does this tool's result render? `update_plan` updates the
            // right-rail Plan widget; every other tool gets a canvas tool
            // card (REQUIREMENTS §2.4, §2.6). The registry decides — the loop
            // does not special-case tool names.
            let to_plan = matches!(
                tools::ui(&name).map(|u| u.result),
                Some(tools::ResultArtifact::Plan)
            );

            // ── PreToolUse hook (M2.5) ─────────────────────────────────
            // CC parity: a `decision: block` (or exit 2) cancels the call.
            // We turn a block into a synthetic error outcome the model can
            // recover from, then skip the actual dispatch.
            let outcome = if st.hooks.has_event(HookEvent::PreToolUse) {
                let payload = pre_tool_use_payload(
                    session_id,
                    turn_id,
                    iteration_count,
                    &call_id,
                    &name,
                    &args_value,
                );
                match st.hooks.trigger(HookEvent::PreToolUse, &name, payload).await {
                    HookOutcome::Block { reason } => {
                        let msg = format!(
                            "tool '{name}' was blocked by a PreToolUse hook: {reason}"
                        );
                        tool_error_count += 1;
                        st.emit(
                            session_id,
                            events::kind::TOOL_LIFECYCLE,
                            tools::tool_artifact(
                                turn_id,
                                iteration_count,
                                &call_id,
                                &name,
                                &args_value,
                                Some(&tools::ToolOutcome::error(msg.clone())),
                            )
                            .into_payload(),
                        )
                        .await;
                        Some(tools::ToolOutcome::error(msg))
                    }
                    HookOutcome::Continue => None,
                }
            } else {
                None
            };

            let outcome = if let Some(o) = outcome {
                o
            } else {
                // Start frame — a canvas tool card only; the plan tool has none.
                if !to_plan {
                    st.emit(
                        session_id,
                        events::kind::TOOL_LIFECYCLE,
                        tools::tool_artifact(
                            turn_id,
                            iteration_count,
                            &call_id,
                            &name,
                            &args_value,
                            None,
                        )
                        .into_payload(),
                    )
                    .await;
                }

                let o = tools::dispatch(
                    &st.http, &st.corpus, &st.skills, &name, &args_value,
                )
                .await;
                if o.is_error {
                    tool_error_count += 1;
                }

                if to_plan {
                    // The plan tool is not a canvas card and not a gate. Emit the
                    // plan only on success — a rejected update leaves it unchanged.
                    if !o.is_error {
                        st.emit(
                            session_id,
                            events::kind::PLAN_UPDATED,
                            plan_payload(turn_id, &o),
                        )
                        .await;
                    }
                } else {
                    st.emit(
                        session_id,
                        events::kind::TOOL_LIFECYCLE,
                        tools::tool_artifact(
                            turn_id,
                            iteration_count,
                            &call_id,
                            &name,
                            &args_value,
                            Some(&o),
                        )
                        .into_payload(),
                    )
                    .await;
                }

                // ── PostToolUse hook (M2.5) — advisory; observe-only ─
                if st.hooks.has_event(HookEvent::PostToolUse) {
                    let payload = post_tool_use_payload(
                        session_id,
                        turn_id,
                        iteration_count,
                        &call_id,
                        &name,
                        &args_value,
                        &o,
                    );
                    let _ = st
                        .hooks
                        .trigger(HookEvent::PostToolUse, &name, payload)
                        .await;
                }
                o
            };

            // Re-inject the call and its result so the next iteration sees
            // the full tool dialog (order matters: call before output). Only
            // `model_output` reaches the model — the display / debug payloads
            // are UI-only (REQUIREMENTS §4.2).
            additional_inputs.push(serde_json::json!({
                "type": "function_call", "call_id": call_id,
                "name": name, "arguments": arguments,
            }));
            additional_inputs.push(serde_json::json!({
                "type": "function_call_output", "call_id": call_id,
                "output": outcome.model_output,
            }));
        }

        if doom_hit {
            stop_reason = "doom_loop".into();
            first_guard.get_or_insert("doom_loop");
            break 'turn;
        }
    }

    // ── finalize: assistant message + metrics + lifecycle events ────────
    let final_text = compose_final_text(&final_reply, &stop_reason, fatal_error.as_deref());
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
            compaction_count: compaction_count as i64,
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
        events::kind::TURN_METRICS_RECORDED,
        serde_json::json!({
            "turn_id": turn_id,
            "stop_reason": stop_reason,
            "first_triggered_guard": first_guard,
            "iteration_count": iteration_count,
            "tool_call_count": tool_call_count,
            "tool_error_count": tool_error_count,
            "compaction_count": compaction_count,
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
        events::kind::ASSISTANT_DONE,
        serde_json::json!({
            "turn_id": turn_id,
            "message_seq": assistant.seq,
            "stop_reason": stop_reason,
        }),
    )
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
                stop_payload(session_id, turn_id, &stop_reason, &final_text),
            )
            .await;
    }

    sessions::touch(&st.pool, session_id).await?;
    tracing::info!(session_id, turn_id, %stop_reason, iteration_count, "agent turn complete");
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
fn plan_payload(turn_id: &str, outcome: &tools::ToolOutcome) -> serde_json::Value {
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
fn build_search_data(action: Option<&WebSearchAction>) -> serde_json::Value {
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
        _ => Some("本回合提前结束。"),
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

fn pre_tool_use_payload(
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

fn post_tool_use_payload(
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

fn pre_compact_payload(session_id: &str, turn_id: &str, tokens_before: u32) -> serde_json::Value {
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
