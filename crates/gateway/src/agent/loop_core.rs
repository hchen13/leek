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
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use futures::StreamExt;
use tokio::sync::Notify;

use crate::api::AppState;
use crate::hooks::{HookEvent, HookOutcome};
use crate::llm::{
    pricing, ChatMessage, ChatRequest, LlmEvent, StopReason, ToolSpec, WebSearchPhase,
};

use super::builtin_governance::{BuiltinTracker, TrackerSignal};
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
    /// M3.2: per-turn user-abort notifier. `Some` for the main agent —
    /// the loop polls it in the inner `select!` alongside the stream
    /// item and the idle-timeout. `None` for a subagent: a user clicks
    /// abort on the *turn*, not on a particular subagent, and the abort
    /// drops the parent loop which in turn drops the subagent task.
    pub abort_signal: Option<Arc<Notify>>,
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
    // M3.1: per-turn duplicate-URL tracker for codex-builtin web_search.
    // L2 abort + L3 next-iter inject — the only leak path leek can plug
    // for codex server-side builtin tools (see `builtin_governance`).
    let mut builtin_tracker = BuiltinTracker::new(
        p.guards.builtin_url_warn_threshold,
        p.guards.builtin_url_abort_threshold,
    );

    let stop_reason: String;
    let mut first_guard: Option<&'static str> = None;
    let mut fatal_error: Option<String> = None;

    'turn: loop {
        // ── pre-iteration guards (identical to main-agent loop) ─────────
        // M3.2: an abort that fired between iters (e.g. while tool
        // dispatch was running, which we don't `select!` over) is
        // observable here as a notify that was already armed. We can't
        // `notified()` without awaiting, so we check the slot's removal
        // via the registry; if the registry has been emptied externally
        // (it isn't currently — but this is the place to add it), we
        // honor that. The endpoint never empties the slot, so today we
        // pre-empt by polling the notify with a zero-timeout to consume
        // a pending wake. Cheap (Notify is a per-task atomic + waker).
        if let Some(notify) = &p.abort_signal {
            // Future::poll_ready style — `tokio::time::timeout(ZERO, _)`
            // is a clean way to "check, then move on" without spinning a
            // separate task. A `Ready` result means the notify was
            // already permitted (`notify_one` had been called before we
            // got here), and we bail.
            if tokio::time::timeout(std::time::Duration::ZERO, notify.notified())
                .await
                .is_ok()
            {
                tracing::info!(
                    session_id = p.session_id,
                    turn_id = p.turn_id,
                    iteration = iteration_count,
                    "turn aborted by user (between iters)"
                );
                stop_reason = "user_aborted".into();
                break 'turn;
            }
        }
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
        // M3.1: drain pending duplicate-URL warnings into the next iter as
        // a `developer` message. The codex Responses API has no reverse
        // channel inside an in-flight iter, so this is the earliest
        // opportunity we can talk to the model about its prior-iter
        // duplicate calls (spec §B). The hint is a nudge, not a command —
        // the model can still pick to keep searching.
        if let Some(hint) = builtin_tracker.drain_for_inject() {
            iter_inputs.push(serde_json::json!({
                "role": "developer",
                "content": hint,
            }));
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
        // M3.1: set inside the stream loop when the builtin tracker fires
        // an Abort signal — read after the stream exits to bail out of the
        // turn with `stop_reason = "codex_duplicate_abort"`. Tracks the
        // offending URL for diagnostics so the assistant_done payload can
        // explain itself if anyone digs into the transcript.
        let mut codex_abort: Option<(String, String, u32)> = None;
        // M3.2: set when the user-abort `Notify` fires inside the stream
        // poll. Like `idle_hit`, we set the flag and break the stream so
        // the post-stream cleanup runs once on its own path. Only the
        // main agent registers an abort signal — a subagent sees `None`
        // and falls into the simpler stream-only path.
        let mut user_aborted = false;
        'stream: loop {
            // The poll fans out to three sources: a stream item, the
            // optional idle timeout, and the optional user-abort notify.
            // We `select!` only when at least one of the latter two is
            // active; otherwise we keep the bare `stream.next().await`
            // so a turn with neither guard nor abort signal stays cheap.
            let item = match (p.guards.idle_timeout, &p.abort_signal) {
                (Some(d), Some(notify)) => {
                    tokio::select! {
                        biased;
                        _ = notify.notified() => {
                            user_aborted = true;
                            break 'stream;
                        }
                        res = tokio::time::timeout(d, stream.next()) => match res {
                            Ok(item) => item,
                            Err(_) => {
                                idle_hit = true;
                                break 'stream;
                            }
                        }
                    }
                }
                (Some(d), None) => match tokio::time::timeout(d, stream.next()).await {
                    Ok(item) => item,
                    Err(_) => {
                        idle_hit = true;
                        break 'stream;
                    }
                },
                (None, Some(notify)) => {
                    tokio::select! {
                        biased;
                        _ = notify.notified() => {
                            user_aborted = true;
                            break 'stream;
                        }
                        item = stream.next() => item,
                    }
                }
                (None, None) => stream.next().await,
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
                    // M3.1: feed the tracker on completion frames — that's
                    // when the action variant + URL are known (start frames
                    // carry `None`, see `web_search_event`). The tracker
                    // ignores empty URLs, so a `search` activity that lacks
                    // a URL (the search-query frame) is a natural no-op.
                    let mut warn_signal: Option<TrackerSignal> = None;
                    if phase == WebSearchPhase::Completed {
                        let (atype, url) = builtin_target(&data);
                        if !atype.is_empty() && !url.is_empty() {
                            warn_signal = builtin_tracker.observe(&atype, &url);
                        }
                    }
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

                    // M3.1: emit the warning event (canvas surface) right
                    // after the search_lifecycle so the frontend renders
                    // them adjacent. Abort signal additionally sets
                    // `codex_abort` so we break the stream below.
                    match warn_signal {
                        Some(TrackerSignal::Warn { action_type, url, count }) => {
                            let mut warn_payload = serde_json::json!({
                                "turn_id": p.turn_id,
                                "iteration": iteration_count,
                                "action_type": action_type,
                                "url": url,
                                "count": count,
                                "threshold": p.guards.builtin_url_warn_threshold,
                            });
                            events::stamp_parent_in_place(
                                &mut warn_payload,
                                p.parent_turn_id,
                                p.depth,
                            );
                            p.st.emit(
                                p.session_id,
                                events::kind::CODEX_DUPLICATE_URL_WARNING,
                                warn_payload,
                            )
                            .await;
                        }
                        Some(TrackerSignal::Abort { action_type, url, count }) => {
                            // Emit a final warning event (kind = same, so the
                            // frontend can render the abort as the last warning
                            // chip with `count >= abort_threshold`) before we
                            // break the stream.
                            let mut warn_payload = serde_json::json!({
                                "turn_id": p.turn_id,
                                "iteration": iteration_count,
                                "action_type": action_type.clone(),
                                "url": url.clone(),
                                "count": count,
                                "threshold": p.guards.builtin_url_abort_threshold,
                                "abort": true,
                            });
                            events::stamp_parent_in_place(
                                &mut warn_payload,
                                p.parent_turn_id,
                                p.depth,
                            );
                            p.st.emit(
                                p.session_id,
                                events::kind::CODEX_DUPLICATE_URL_WARNING,
                                warn_payload,
                            )
                            .await;
                            codex_abort = Some((action_type, url, count));
                            break 'stream;
                        }
                        None => {}
                    }
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
        // M3.2: user clicked abort mid-stream. We drop `stream` by
        // letting it fall out of scope at loop end (the codex client
        // closes the upstream HTTP connection on drop), keep whatever
        // text we'd already accumulated this iter for the partial reply
        // (the finalize path appends a stop note), and break the turn.
        // No first_guard — abort is user-triggered, not a guard trip,
        // so the metrics row reads as `stop_reason=user_aborted,
        // first_triggered_guard=null` and a downstream dashboard can
        // tell user aborts apart from guard hits.
        if user_aborted {
            tracing::info!(
                session_id = p.session_id,
                turn_id = p.turn_id,
                iteration = iteration_count,
                "turn aborted by user (mid-stream)"
            );
            final_reply = iter_text;
            stop_reason = "user_aborted".into();
            break 'turn;
        }
        // M3.1: codex builtin web_search hit the abort threshold. Bail with
        // a typed stop_reason so the finalize path can render a clear note
        // ("检测到 codex 内置 web_search 重复 open 同一 URL"). We discard
        // `iter_text` because any partial text written this iter was
        // followed by a search-side runaway — emitting it would mislead.
        if let Some((action_type, url, count)) = codex_abort {
            tracing::warn!(
                session_id = p.session_id,
                turn_id = p.turn_id,
                action_type,
                url,
                count,
                "codex builtin web_search abort threshold tripped"
            );
            final_reply = iter_text;
            stop_reason = "codex_duplicate_abort".into();
            first_guard.get_or_insert("codex_duplicate_abort");
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
                    &p.st.vendors,
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

/// M3.1: pull the `(action_type, url)` pair from a `search_lifecycle` data
/// blob to feed the duplicate-URL tracker. Returns `("", "")` for shapes
/// the tracker cannot key off (a `search` activity without a single URL
/// to attribute, the `Unknown` variant, …) — the caller treats empty as
/// "do not observe".
///
/// The `search` activity intentionally collapses to its `query` (since it
/// can return many URLs and the tracker is per-URL); the tracker keys are
/// `("search", query)` so a model that re-issues the *same query* across
/// iters still trips. `open_page` / `find_in_page` key off their `url`.
fn builtin_target(data: &serde_json::Value) -> (String, String) {
    let action_type = data
        .get("action_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let url = match action_type.as_str() {
        "open_page" | "find_in_page" => data
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "search" => data
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    };
    (action_type, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_target_extracts_open_page_url() {
        let data = serde_json::json!({
            "action_type": "open_page",
            "url": "https://a.com/x.pdf",
        });
        assert_eq!(
            builtin_target(&data),
            ("open_page".into(), "https://a.com/x.pdf".into()),
        );
    }

    #[test]
    fn builtin_target_extracts_find_in_page_url() {
        let data = serde_json::json!({
            "action_type": "find_in_page",
            "url": "https://a.com/x.pdf",
            "pattern": "foo",
        });
        assert_eq!(
            builtin_target(&data),
            ("find_in_page".into(), "https://a.com/x.pdf".into()),
        );
    }

    #[test]
    fn builtin_target_uses_query_for_search() {
        let data = serde_json::json!({
            "action_type": "search",
            "query": "长电科技 复盘",
        });
        assert_eq!(
            builtin_target(&data),
            ("search".into(), "长电科技 复盘".into()),
        );
    }

    #[test]
    fn builtin_target_empty_when_no_action_type() {
        let data = serde_json::json!({});
        assert_eq!(builtin_target(&data), ("".into(), "".into()));
    }

    #[test]
    fn builtin_target_empty_for_unknown_variant() {
        let data = serde_json::json!({ "action_type": "ai_dance_party" });
        assert_eq!(
            builtin_target(&data),
            ("ai_dance_party".into(), "".into()),
        );
    }

    /// End-to-end shape test for the M3.1 inject path: simulate a stream of
    /// `search_lifecycle` data blobs, drive them through `builtin_target +
    /// BuiltinTracker`, then assert the inject hint and the abort signal
    /// match what `run_loop` actually feeds into the next iter / break
    /// path. The real `run_loop` is async + needs a `CodexClient`, which is
    /// not mock-friendly without restructuring; this test pins the algorithm
    /// shape so a regression on tracker / target / inject formatting is
    /// caught even without an end-to-end harness.
    #[test]
    fn replay_drives_warn_inject_then_abort() {
        // warn=3, abort=5 — tighter than defaults so the test runs in a
        // handful of synthetic events.
        let mut tracker = BuiltinTracker::new(3, 5);

        // Iter 1: codex opens URL X four times — warn at the 3rd.
        let url = "https://static.cninfo.com.cn/finalpage/666.PDF";
        let event = serde_json::json!({ "action_type": "open_page", "url": url });

        let (a, u) = builtin_target(&event);
        assert_eq!((a.as_str(), u.as_str()), ("open_page", url));

        // 1st + 2nd observations: under threshold → None.
        assert!(tracker.observe(&a, &u).is_none());
        assert!(tracker.observe(&a, &u).is_none());
        // 3rd: cross warn → Warn signal (loop emits canvas warning).
        let warn = tracker.observe(&a, &u).unwrap();
        match &warn {
            TrackerSignal::Warn { count, .. } => assert_eq!(*count, 3),
            other => panic!("expected Warn, got {other:?}"),
        }
        // 4th: warn already fired this iter — None (warned set holds).
        assert!(tracker.observe(&a, &u).is_none());

        // Iter boundary — drain produces the developer-role inject we'd
        // push onto next iter's input.
        let hint = tracker.drain_for_inject().expect("warn → hint");
        assert!(hint.contains("[open_page]"));
        assert!(hint.contains(url));
        // The loop wraps the hint in a developer message — verify the
        // shape we push into `iter_inputs` is what build_request_body
        // accepts (raw input items, `role/content` pair).
        let inject = serde_json::json!({ "role": "developer", "content": &hint });
        assert_eq!(inject["role"], "developer");
        assert!(inject["content"].as_str().unwrap().contains(url));

        // Iter 2: same URL hit twice more (count goes 4, then 5) — count 5
        // crosses abort threshold (5), so the second observe returns Abort.
        // Note: 4th observe was inside iter 1 and returned None *because of
        // warn dedupe*, not because the count didn't increment. The count
        // really is at 4 after iter 1, so the next observe brings it to 5.
        assert!(matches!(
            tracker.observe(&a, &u),
            Some(TrackerSignal::Abort { count: 5, .. })
        ));
        // After Abort, subsequent observations are silent — loop has
        // already broken the stream and set stop_reason.
        assert!(tracker.observe(&a, &u).is_none());
    }

    /// M3.2: pin the select! shape used by the per-iter stream poll —
    /// when the user-abort notify fires before the next stream item, we
    /// break the stream with `user_aborted=true` (the loop then turns
    /// that into `stop_reason="user_aborted"`). Mirrors the inner block
    /// of `run_loop`'s `(None, Some(notify))` arm with a synthetic mpsc
    /// "stream" — the real stream type is a `BoxStream` from `codex`
    /// that we cannot easily synthesize, but the select! arm shape is
    /// the part we changed and the part that can break.
    #[tokio::test]
    async fn user_abort_wins_select_over_pending_stream() {
        use tokio::sync::mpsc;
        let (_tx, mut rx) = mpsc::channel::<Result<LlmEvent>>(8);
        let notify = Arc::new(Notify::new());
        let notify_for_fire = notify.clone();

        // Fire the abort after a short delay so the `select!` is
        // already armed when notify_one() lands.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            notify_for_fire.notify_one();
        });

        let mut user_aborted = false;
        let mut item_seen = false;
        tokio::select! {
            biased;
            _ = notify.notified() => {
                user_aborted = true;
            }
            item = rx.recv() => {
                item_seen = item.is_some();
            }
        }

        assert!(user_aborted, "notify should have won the select");
        assert!(!item_seen, "no stream item was sent");
    }

    /// M3.2: when a stream item is already pending and the notify hasn't
    /// fired, the stream arm wins. Verifies the `biased` ordering does NOT
    /// starve normal stream items — `biased` only matters when multiple
    /// arms are simultaneously ready; here only `recv()` is ready.
    #[tokio::test]
    async fn pending_stream_item_wins_when_notify_idle() {
        use tokio::sync::mpsc;
        let (tx, mut rx) = mpsc::channel::<Result<LlmEvent>>(8);
        let notify = Arc::new(Notify::new());

        tx.send(Ok(LlmEvent::Ping)).await.unwrap();

        let mut user_aborted = false;
        let mut got_ping = false;
        tokio::select! {
            biased;
            _ = notify.notified() => {
                user_aborted = true;
            }
            item = rx.recv() => {
                got_ping = matches!(item, Some(Ok(LlmEvent::Ping)));
            }
        }

        assert!(!user_aborted, "notify never fired");
        assert!(got_ping, "the pre-sent Ping should have been received");
    }

    /// M3.2: between-iter abort check. The pre-iter probe uses a zero
    /// timeout on `notified()` to consume an already-armed wake without
    /// awaiting. Pin that shape — if `notify_one()` was called before
    /// the probe, the probe returns Ok immediately and the loop bails.
    #[tokio::test]
    async fn zero_timeout_probe_catches_already_armed_notify() {
        let notify = Arc::new(Notify::new());
        notify.notify_one();
        let armed = tokio::time::timeout(std::time::Duration::ZERO, notify.notified())
            .await
            .is_ok();
        assert!(armed, "an already-permitted notify must resolve under ZERO timeout");
    }

    /// And the negative case: with no prior `notify_one`, the probe must
    /// time out (the loop continues into the next iter, as intended).
    #[tokio::test]
    async fn zero_timeout_probe_skips_when_notify_idle() {
        let notify = Arc::new(Notify::new());
        let armed = tokio::time::timeout(std::time::Duration::ZERO, notify.notified())
            .await
            .is_ok();
        assert!(!armed, "an unarmed notify must NOT resolve under ZERO timeout");
    }
}
