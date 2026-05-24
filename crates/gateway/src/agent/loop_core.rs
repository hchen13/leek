//! The shared inner loop: model → tool calls → re-inject → repeat.
//!
//! M1's `drive` had this loop inline; M2.7 extracts it so the main agent
//! and any subagent share *one* loop implementation (spec §D: "loop 代码
//! 不复制"). The two callers differ in:
//!
//! - Lifecycle wrapping. Main agent loads session history, persists the
//!   assistant message, fires SessionStart / UserPromptSubmit / Stop
//!   hooks. Subagent gets a single input message, fires no chat-message
//!   hooks, returns the assistant text to its caller as a tool result.
//! - Event tagging. Subagent emits the same `tool_lifecycle` /
//!   `note_trace` / `search_lifecycle` event kinds, but stamps each
//!   payload with `parent_turn_id` so the frontend routes them under a
//!   `subagent_card` (spec §G) instead of pushing them flat onto the
//!   parent canvas.
//! - Tool surface. Subagent restricts dispatch to `AgentDef.allowed_tools`
//!   (an empty list = full surface). Main agent always uses the full
//!   surface.
//!
//! Everything else — guards, auto-compaction, hooks (PreToolUse /
//! PostToolUse / PreCompact), doom-loop detection, token counting, cost
//! cap, transcript routing — runs identically. A subagent inherits every
//! safety net the main agent has.

use std::collections::VecDeque;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;

use crate::api::AppState;
use crate::hooks::{HookEvent, HookOutcome};
use crate::llm::{
    pricing, ChatMessage, ChatRequest, LlmEvent, StopReason, ToolSpec, WebSearchPhase,
};

use super::compaction;
use super::events;
use super::guards::{self, GuardConfig};
use super::tools;

/// One run of the inner loop — what the caller hands in.
pub struct LoopParams<'a> {
    pub st: &'a AppState,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    /// Main agent: `None`. Subagent: the parent turn's id, so events the
    /// loop emits can be routed into the parent's `subagent_card`.
    pub parent_turn_id: Option<&'a str>,
    /// 0 for main agent, 1 for first-level subagent, 2 for grand-subagent.
    pub depth: u32,
    /// System prompt — fully composed by the caller. The main agent's
    /// builder is `prompt::build_system_prompt`; a subagent's is its
    /// `AgentDef.system_prompt` plus a small "you are a subagent" preamble.
    pub system: String,
    /// Conversation history. Main agent: full session history. Subagent:
    /// one user-role message carrying the parent's `input`.
    pub messages: Vec<ChatMessage>,
    /// Tool specs the model sees. Main agent: full surface. Subagent:
    /// surface filtered by `AgentDef.allowed_tools`.
    pub tool_specs: Vec<ToolSpec>,
    /// Tool *dispatch* allow-list (independent of the model-facing
    /// `tool_specs`, although the caller usually keeps them in sync). A
    /// tool call whose name is outside this set short-circuits to an
    /// error outcome — defense in depth in case the model invents a tool
    /// name that wasn't in its surface.
    pub allowed_tools: ToolAllowlist,
    /// Guard config. Always resolved by the caller from the live settings
    /// snapshot so a subagent shares the same caps as the parent turn.
    pub guards: GuardConfig,
}

/// Which tools the inner loop is willing to dispatch. Constructed once
/// per loop invocation; the dispatcher checks each call against it.
pub enum ToolAllowlist {
    /// All registered tools allowed (main-agent default).
    All,
    /// Only the named tools allowed (subagent with a non-empty
    /// `AgentDef.allowed_tools`).
    Only(std::collections::HashSet<String>),
}

impl ToolAllowlist {
    pub fn permits(&self, name: &str) -> bool {
        match self {
            ToolAllowlist::All => true,
            ToolAllowlist::Only(set) => set.contains(name),
        }
    }
}

/// What the inner loop produces. The caller decides how to persist a
/// chat message (main agent writes the `messages` row; subagent returns
/// the text inline to its caller).
pub struct LoopOutcome {
    pub final_reply: String,
    pub stop_reason: String,
    pub first_guard: Option<&'static str>,
    pub fatal_error: Option<String>,
    pub iteration_count: usize,
    pub tool_call_count: usize,
    pub tool_error_count: usize,
    pub compaction_count: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: chrono::DateTime<chrono::Utc>,
    pub wall_clock_ms: i64,
}

/// Drive the inner loop. Caller owns lifecycle bookkeeping around it —
/// history loading, assistant-message persistence, lifecycle hooks
/// (SessionStart / UserPromptSubmit / Stop), and the `turn_metrics`
/// insert. The loop itself only fires the per-call hooks (PreToolUse /
/// PostToolUse / PreCompact) and emits canvas events.
pub async fn run_loop(mut p: LoopParams<'_>) -> Result<LoopOutcome> {
    let started = Instant::now();
    let started_at = chrono::Utc::now();
    let wall_deadline = p.guards.wall_clock.map(|d| started + d);

    // Auto-compaction trigger — same arithmetic as the main loop.
    let context_window = p
        .guards
        .context_window
        .unwrap_or_else(|| pricing::context_window(super::MODEL));
    let compact_trigger = (p.guards.auto_compact_threshold as f64 * context_window as f64) as u32;

    let mut additional_inputs: Vec<serde_json::Value> = Vec::new();
    let mut final_reply = String::new();
    let mut iteration_count: usize = 0;
    let mut transcript_iter: i64 = 0;
    let mut tool_call_count: usize = 0;
    let mut tool_error_count: usize = 0;
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cost_usd: f64 = 0.0;
    let mut doom_window: VecDeque<(String, String)> = VecDeque::new();
    let mut compaction_summary: Option<String> = None;
    let mut compaction_count: usize = 0;

    let stop_reason: String;
    let mut first_guard: Option<&'static str> = None;
    let mut fatal_error: Option<String> = None;

    'turn: loop {
        // ── pre-iteration guards (identical to main-agent loop) ─────────
        if wall_deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
            stop_reason = "wall_clock_exceeded".into();
            first_guard.get_or_insert("wall_clock_exceeded");
            break 'turn;
        }
        if p.guards
            .max_iterations
            .map(|c| iteration_count >= c)
            .unwrap_or(false)
        {
            stop_reason = "max_iterations".into();
            first_guard.get_or_insert("max_iterations");
            break 'turn;
        }
        if p.guards.cost_cap_usd.map(|c| cost_usd >= c).unwrap_or(false) {
            let cap = p.guards.cost_cap_usd.unwrap_or(0.0);
            p.st.emit(
                p.session_id,
                events::kind::TURN_COST_CAPPED,
                events::stamp_parent(
                    serde_json::json!({
                        "turn_id": p.turn_id,
                        "cap_usd": cap,
                        "actual_cost_usd": cost_usd,
                        "iter_count": iteration_count,
                    }),
                    p.parent_turn_id,
                    p.depth,
                ),
            )
            .await;
            stop_reason = "cost_cap_exceeded".into();
            first_guard.get_or_insert("cost_cap_exceeded");
            break 'turn;
        }

        // ── auto-compaction (same code path as main loop) ───────────────
        if compact_trigger > 0 {
            let ctx_tokens = compaction::estimate_context_tokens(
                &p.system,
                compaction_summary.as_deref(),
                &p.messages,
                &additional_inputs,
                &p.tool_specs,
            );
            if ctx_tokens >= compact_trigger {
                if p.st.hooks.has_event(HookEvent::PreCompact) {
                    let _ = p
                        .st
                        .hooks
                        .trigger(
                            HookEvent::PreCompact,
                            "auto",
                            super::pre_compact_payload(p.session_id, p.turn_id, ctx_tokens),
                        )
                        .await;
                }
                transcript_iter += 1;
                match compaction::compact(
                    p.st,
                    p.session_id,
                    p.turn_id,
                    &p.system,
                    &p.tool_specs,
                    compaction::TurnContext {
                        summary: &mut compaction_summary,
                        messages: &mut p.messages,
                        tool_dialog: &mut additional_inputs,
                    },
                    ctx_tokens,
                    transcript_iter,
                    p.guards.idle_timeout,
                )
                .await
                {
                    Ok(compaction::Compacted::Done) => compaction_count += 1,
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

        let mut iter_messages: Vec<ChatMessage> = Vec::with_capacity(p.messages.len() + 1);
        if let Some(summary) = &compaction_summary {
            iter_messages.push(compaction::summary_message(summary));
        }
        iter_messages.extend(p.messages.iter().cloned());

        let req = ChatRequest {
            model: super::MODEL.to_string(),
            system: p.system.clone(),
            messages: iter_messages,
            tools: p.tool_specs.clone(),
            additional_inputs: iter_inputs,
            reasoning_effort: Some(super::REASONING_EFFORT.to_string()),
            verbosity: Some(super::VERBOSITY.to_string()),
            web_search: p.st.web_search,
            session_id: p.session_id.to_string(),
            turn_id: p.turn_id.to_string(),
            iteration: transcript_iter,
        };

        let mut stream = match p.st.codex.chat(req).await {
            Ok(s) => s,
            Err(e) => {
                fatal_error = Some(e.to_string());
                stop_reason = "fatal_error".into();
                break 'turn;
            }
        };

        let mut iter_text = String::new();
        let mut pending: Vec<(String, String, String)> = Vec::new();
        let mut iter_stop: Option<StopReason> = None;
        let mut idle_hit = false;
        'stream: loop {
            let item = match p.guards.idle_timeout {
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
                    // Subagent deltas would clobber the main agent's chat
                    // streaming bubble — suppress them entirely (we still
                    // accumulate text locally for the final result). Main
                    // agent emits as before.
                    if p.parent_turn_id.is_none() {
                        p.st.emit_ephemeral(
                            p.session_id,
                            events::kind::ASSISTANT_DELTA,
                            serde_json::json!({
                                "turn_id": p.turn_id,
                                "iteration": iteration_count,
                                "text": text,
                            }),
                        )
                        .await;
                    }
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
                    let canvas_phase = match phase {
                        WebSearchPhase::Started => events::Phase::Start,
                        WebSearchPhase::Completed => events::Phase::Completion,
                    };
                    let data = super::build_search_data(action.as_ref());
                    let mut payload = events::CanvasArtifact::search(
                        p.turn_id,
                        iteration_count,
                        &call_id,
                        canvas_phase,
                        data,
                    )
                    .into_payload();
                    events::stamp_parent_in_place(&mut payload, p.parent_turn_id, p.depth);
                    p.st.emit(p.session_id, events::kind::SEARCH_LIFECYCLE, payload)
                        .await;
                }
                Ok(LlmEvent::Usage(u)) => {
                    input_tokens += u.input_tokens as u64;
                    output_tokens += u.output_tokens as u64;
                    cost_usd += pricing::compute_cost(super::MODEL, &u);
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

        // ── no tool calls → final reply ─────────────────────────────────
        if pending.is_empty() {
            final_reply = iter_text;
            stop_reason = match iter_stop {
                Some(StopReason::MaxTokens) => "max_tokens".into(),
                _ => "end_turn".into(),
            };
            break 'turn;
        }

        let note = iter_text.trim();
        if !note.is_empty() {
            let mut payload =
                events::CanvasArtifact::note(p.turn_id, iteration_count, note).into_payload();
            events::stamp_parent_in_place(&mut payload, p.parent_turn_id, p.depth);
            p.st.emit(p.session_id, events::kind::NOTE_TRACE, payload).await;
        }

        // ── dispatch tool calls, re-inject results, loop ────────────────
        let mut doom_hit = false;
        for (call_id, name, arguments) in pending {
            tool_call_count += 1;

            if let Some(threshold) = p.guards.doom_loop_threshold {
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

            let to_plan = matches!(
                tools::ui(&name).map(|u| u.result),
                Some(tools::ResultArtifact::Plan)
            );

            // Tool-surface enforcement — defense in depth for subagents
            // (the model only saw allowed tools in its spec, so this
            // should rarely fire, but a misbehaving model can still
            // invent a tool name).
            let surface_block = if !p.allowed_tools.permits(&name) {
                Some(format!(
                    "tool '{name}' is not in this agent's allow-list"
                ))
            } else {
                None
            };

            // ── PreToolUse hook ─────────────────────────────────────
            let pre_outcome = if surface_block.is_some() {
                None // surface block beats hook check
            } else if p.st.hooks.has_event(HookEvent::PreToolUse) {
                let payload = super::pre_tool_use_payload(
                    p.session_id,
                    p.turn_id,
                    iteration_count,
                    &call_id,
                    &name,
                    &args_value,
                );
                match p.st.hooks.trigger(HookEvent::PreToolUse, &name, payload).await {
                    HookOutcome::Block { reason } => {
                        let msg = format!(
                            "tool '{name}' was blocked by a PreToolUse hook: {reason}"
                        );
                        tool_error_count += 1;
                        let mut tlp = tools::tool_artifact(
                            p.turn_id,
                            iteration_count,
                            &call_id,
                            &name,
                            &args_value,
                            Some(&tools::ToolOutcome::error(msg.clone())),
                        )
                        .into_payload();
                        events::stamp_parent_in_place(&mut tlp, p.parent_turn_id, p.depth);
                        p.st.emit(p.session_id, events::kind::TOOL_LIFECYCLE, tlp).await;
                        Some(tools::ToolOutcome::error(msg))
                    }
                    HookOutcome::Continue => None,
                }
            } else {
                None
            };

            let outcome = if let Some(msg) = surface_block {
                tool_error_count += 1;
                let err = tools::ToolOutcome::error(msg);
                let mut tlp = tools::tool_artifact(
                    p.turn_id,
                    iteration_count,
                    &call_id,
                    &name,
                    &args_value,
                    Some(&err),
                )
                .into_payload();
                events::stamp_parent_in_place(&mut tlp, p.parent_turn_id, p.depth);
                p.st.emit(p.session_id, events::kind::TOOL_LIFECYCLE, tlp).await;
                err
            } else if let Some(o) = pre_outcome {
                o
            } else {
                if !to_plan {
                    let mut tlp = tools::tool_artifact(
                        p.turn_id,
                        iteration_count,
                        &call_id,
                        &name,
                        &args_value,
                        None,
                    )
                    .into_payload();
                    events::stamp_parent_in_place(&mut tlp, p.parent_turn_id, p.depth);
                    p.st.emit(p.session_id, events::kind::TOOL_LIFECYCLE, tlp).await;
                }

                let ctx = tools::DispatchCtx {
                    st: Some(p.st),
                    session_id: p.session_id,
                    parent_turn_id: p.turn_id,
                    parent_depth: p.depth,
                };
                let o = tools::dispatch(
                    &ctx,
                    &p.st.http,
                    &p.st.corpus,
                    &p.st.skills,
                    &p.st.agents,
                    &name,
                    &args_value,
                )
                .await;
                if o.is_error {
                    tool_error_count += 1;
                }

                if to_plan {
                    if !o.is_error {
                        // Plan widget is owned by the main agent only — a
                        // subagent's `update_plan` would clobber it. We
                        // still let the tool *run* (it has no canvas card
                        // and just returns plan JSON), but suppress the
                        // event so the right-rail stays parent-owned.
                        if p.parent_turn_id.is_none() {
                            p.st.emit(
                                p.session_id,
                                events::kind::PLAN_UPDATED,
                                super::plan_payload(p.turn_id, &o),
                            )
                            .await;
                        }
                    }
                } else {
                    let mut tlp = tools::tool_artifact(
                        p.turn_id,
                        iteration_count,
                        &call_id,
                        &name,
                        &args_value,
                        Some(&o),
                    )
                    .into_payload();
                    events::stamp_parent_in_place(&mut tlp, p.parent_turn_id, p.depth);
                    p.st.emit(p.session_id, events::kind::TOOL_LIFECYCLE, tlp).await;
                }

                if p.st.hooks.has_event(HookEvent::PostToolUse) {
                    let payload = super::post_tool_use_payload(
                        p.session_id,
                        p.turn_id,
                        iteration_count,
                        &call_id,
                        &name,
                        &args_value,
                        &o,
                    );
                    let _ = p
                        .st
                        .hooks
                        .trigger(HookEvent::PostToolUse, &name, payload)
                        .await;
                }
                o
            };

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

    let ended_at = chrono::Utc::now();
    let wall_clock_ms = started.elapsed().as_millis() as i64;

    Ok(LoopOutcome {
        final_reply,
        stop_reason,
        first_guard,
        fatal_error,
        iteration_count,
        tool_call_count,
        tool_error_count,
        compaction_count,
        input_tokens,
        output_tokens,
        cost_usd,
        started_at,
        ended_at,
        wall_clock_ms,
    })
}
