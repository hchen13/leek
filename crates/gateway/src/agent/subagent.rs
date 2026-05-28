//! Subagent dispatcher — the spawn site for a child agent loop (M2.7).
//!
//! Called by the `task` tool. Resolves the requested `agent_name`
//! against `AgentRegistry`, builds a minimal `LoopParams` from the
//! `AgentDef`, runs `loop_core::run_loop`, persists a `turn_metrics`
//! row (with `parent_turn_id` + `depth` set), and returns the
//! subagent's final assistant text to the caller.
//!
//! Spec invariants (§D, §E, §F, §G):
//!
//! - **Same loop**. The inner loop is `loop_core::run_loop` — exactly the
//!   one the main agent runs. Guards, hooks, cost cap, auto-compaction,
//!   doom-loop detection: all inherited.
//! - **Depth cap**. `depth >= 2` → refuse with an explicit error. A
//!   parent at depth 2 can still *call* `task`, but the dispatcher
//!   short-circuits before any model call.
//! - **Tool surface**. `AgentDef.allowed_tools` (when non-empty) is the
//!   model-facing spec list **and** the dispatch allow-list. Empty list
//!   = full surface, mirroring the main agent.
//! - **Event routing**. Every event the subagent loop emits carries
//!   `parent_turn_id` so the frontend collapses it into the parent's
//!   `subagent_card`. A dedicated `subagent_lifecycle` event brackets
//!   the spawn / return for the card itself.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;

use crate::agents::{AgentDef, AgentRegistry};
use crate::api::AppState;
use crate::llm::{ChatMessage, Role};
use crate::vault::turn_metrics;

use super::events;
use super::guards::GuardConfig;
use super::loop_core::{self, LoopParams, ToolAllowlist};
use super::tools;

/// What the dispatcher hands the `task` tool to return to the model.
/// `result` is the subagent's final assistant text (the "digest"); the
/// rest is metadata for the canvas card and the parent's logbook.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub agent_name: String,
    pub subagent_turn_id: String,
    pub result: String,
    pub stop_reason: String,
    pub iteration_count: usize,
    pub tool_call_count: usize,
    pub cost_usd: f64,
    pub wall_clock_ms: i64,
    /// `true` when the subagent ended via fatal_error or a non-natural
    /// guard. The model recovers by reading the digest (which carries
    /// any partial work plus a `[本回合提前结束: ...]` tail).
    pub is_error: bool,
}

/// Hard depth limit. depth=0 is the main agent; depth=1 is the first
/// subagent spawn (the main case); depth=2 is the deepest a subagent
/// can spawn. depth=3 would be cap-violating — refused.
pub const MAX_DEPTH: u32 = 2;

/// Resolve an agent name + spawn its loop. The caller is inside the
/// `task` tool's dispatch path. `parent_depth` is the depth of the
/// turn that's calling `task`; the spawned subagent runs at
/// `parent_depth + 1`.
pub async fn spawn(
    st: &AppState,
    session_id: &str,
    parent_turn_id: &str,
    parent_depth: u32,
    agent_name: &str,
    input: &str,
) -> SubagentResult {
    let spawn_depth = parent_depth + 1;
    let subagent_turn_id = format!("{parent_turn_id}.sub-{}", short_id());

    // ── depth cap ───────────────────────────────────────────────────
    if spawn_depth > MAX_DEPTH {
        let msg = format!(
            "Maximum subagent depth ({MAX_DEPTH}) reached — task spawn refused at depth {spawn_depth}"
        );
        emit_lifecycle(
            st,
            session_id,
            parent_turn_id,
            &subagent_turn_id,
            agent_name,
            input,
            spawn_depth,
            LifecyclePhase::Error { reason: msg.clone() },
        )
        .await;
        return SubagentResult {
            agent_name: agent_name.to_string(),
            subagent_turn_id,
            result: msg,
            stop_reason: "max_depth".into(),
            iteration_count: 0,
            tool_call_count: 0,
            cost_usd: 0.0,
            wall_clock_ms: 0,
            is_error: true,
        };
    }

    // ── resolve agent ───────────────────────────────────────────────
    let agent = match resolve_agent(&st.agents, agent_name) {
        Ok(a) => a,
        Err(msg) => {
            emit_lifecycle(
                st,
                session_id,
                parent_turn_id,
                &subagent_turn_id,
                agent_name,
                input,
                spawn_depth,
                LifecyclePhase::Error { reason: msg.clone() },
            )
            .await;
            return SubagentResult {
                agent_name: agent_name.to_string(),
                subagent_turn_id,
                result: msg,
                stop_reason: "agent_not_found".into(),
                iteration_count: 0,
                tool_call_count: 0,
                cost_usd: 0.0,
                wall_clock_ms: 0,
                is_error: true,
            };
        }
    };

    // ── start frame ─────────────────────────────────────────────────
    emit_lifecycle(
        st,
        session_id,
        parent_turn_id,
        &subagent_turn_id,
        &agent.name,
        input,
        spawn_depth,
        LifecyclePhase::Start,
    )
    .await;

    // ── tool surface ────────────────────────────────────────────────
    // Build the surface from the full registry, then filter to
    // `agent.allowed_tools`. Empty allow-list → full surface.
    let full_specs = tools::specs(&st.skills, &st.agents);
    let (tool_specs, allowed_tools) = if agent.allowed_tools.is_empty() {
        (full_specs.clone(), ToolAllowlist::All)
    } else {
        let allow: HashSet<String> = agent.allowed_tools.iter().cloned().collect();
        let filtered: Vec<_> = full_specs
            .into_iter()
            .filter(|s| allow.contains(&s.name))
            .collect();
        (filtered, ToolAllowlist::Only(allow))
    };

    // ── system prompt ───────────────────────────────────────────────
    // The AgentDef body becomes the system prompt verbatim. A short
    // header tells the model it's a subagent and tags the depth so it
    // knows whether it can spawn another task (depth=2 cannot).
    let header = format!(
        "你正在作为 subagent `{}` 运行（depth={}, 父 turn={}）。\
         你的可见工具集已由父 agent 收紧到 AGENT.md 声明的子集。\
         本次任务的 input 在 user message 里。\n\n",
        agent.name, spawn_depth, parent_turn_id
    );
    let mut system = String::with_capacity(header.len() + agent.system_prompt.len());
    system.push_str(&header);
    system.push_str(&agent.system_prompt);

    // ── initial message: the parent's task input as a user prompt ──
    let messages = vec![ChatMessage {
        role: Role::User,
        content: input.to_string(),
    }];

    // Start from the main agent's guard set, then layer the subagent's
    // AGENT.md overrides on top (M3.6 §E). Today only `cost_cap_usd`
    // is overridable per subagent — see `apply_agent_overrides`.
    let mut guards = GuardConfig::resolve(&st.config_snapshot());
    apply_agent_overrides(&mut guards, &agent);

    // M3.6 §F: enforce the AGENT.md allow-list against codex's builtin
    // web_search tool. The model-facing tool surface above already
    // filtered our custom tools to `agent.allowed_tools`, but the codex
    // backend offers `web_search` as a *built-in* tool that lives outside
    // the leek tool surface — without this override, a `corpus-expert`
    // subagent with `allowed_tools: [corpus_search, corpus_read]` would
    // still happily hit `web_search` via the builtin. We pass the
    // request-level capability flag down so loop_core's ChatRequest
    // omits the builtin tool. The main agent's setting (st.web_search)
    // is the ceiling — a subagent never gets a capability the main
    // agent doesn't have.
    let web_search = st.web_search && subagent_web_search_allowed(&agent);

    let params = LoopParams {
        st,
        session_id,
        turn_id: &subagent_turn_id,
        parent_turn_id: Some(parent_turn_id),
        depth: spawn_depth,
        system,
        messages,
        tool_specs,
        allowed_tools,
        guards,
        // M3.2: subagents are NOT independently user-abortable. The user
        // clicks abort on the parent turn; the parent's `select!` wakes,
        // its `'turn` loop breaks, and the spawn future is dropped — which
        // drops the subagent's stream and ends its loop cooperatively.
        abort_signal: None,
        web_search,
        // M4.1.7: subagents are stateless across the parent turn — the
        // parent's tool dialog is OPAQUE to a freshly-spawned subagent
        // (it gets a single user-role message with the parent's `input`
        // text and starts fresh). Subagent's own dialog is dropped
        // here too; only its digest comes back to the parent via
        // `outcome.final_reply`.
        prior_tool_dialog: Vec::new(),
    };

    // `loop_core::run_loop` may indirectly call `task::run` → `spawn` →
    // `run_loop` again (the depth=2 nesting case). The compiler needs us
    // to break the recursive async-future type with `Box::pin`.
    let outcome = match Box::pin(loop_core::run_loop(params)).await {
        Ok(o) => o,
        Err(e) => {
            let msg = format!("subagent loop failed at the storage layer: {e}");
            emit_lifecycle(
                st,
                session_id,
                parent_turn_id,
                &subagent_turn_id,
                &agent.name,
                input,
                spawn_depth,
                LifecyclePhase::Error { reason: msg.clone() },
            )
            .await;
            return SubagentResult {
                agent_name: agent.name.clone(),
                subagent_turn_id,
                result: msg,
                stop_reason: "fatal_error".into(),
                iteration_count: 0,
                tool_call_count: 0,
                cost_usd: 0.0,
                wall_clock_ms: 0,
                is_error: true,
            };
        }
    };

    // ── persist turn_metrics with parent linkage ────────────────────
    // M3.3: the subagent loop produces the same typed FatalReason as the
    // main agent; persist its kind / detail so a subagent dive in the
    // workbench shows the same hint as the main turn would.
    let fatal_reason_kind = outcome.fatal_reason.as_ref().map(|r| r.kind().to_string());
    let fatal_reason_detail = outcome.fatal_reason.as_ref().map(|r| r.detail());

    if let Err(e) = turn_metrics::insert(
        &st.pool,
        &turn_metrics::NewTurnMetrics {
            turn_id: &subagent_turn_id,
            session_id,
            model: super::MODEL,
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
            parent_turn_id: Some(parent_turn_id),
            depth: spawn_depth as i64,
        },
    )
    .await
    {
        // We still want the result to flow back to the parent — a
        // metrics persist failure is logged but not surfaced as a
        // SubagentResult error.
        tracing::error!(
            error = %e,
            subagent_turn_id,
            parent_turn_id,
            "failed to persist subagent turn_metrics"
        );
    }

    let final_text = compose_subagent_final_text(
        &outcome.final_reply,
        &outcome.stop_reason,
        outcome.fatal_error.as_deref(),
    );

    let is_error =
        outcome.stop_reason == "fatal_error" || outcome.fatal_error.is_some();

    emit_lifecycle(
        st,
        session_id,
        parent_turn_id,
        &subagent_turn_id,
        &agent.name,
        input,
        spawn_depth,
        LifecyclePhase::Completion {
            result_preview: preview(&final_text),
            outcome: &outcome,
            agent: &agent,
        },
    )
    .await;

    SubagentResult {
        agent_name: agent.name.clone(),
        subagent_turn_id,
        result: final_text,
        stop_reason: outcome.stop_reason,
        iteration_count: outcome.iteration_count,
        tool_call_count: outcome.tool_call_count,
        cost_usd: outcome.cost_usd,
        wall_clock_ms: outcome.wall_clock_ms,
        is_error,
    }
}

/// M3.6 §F: should this subagent's loop offer codex's builtin
/// `web_search` tool? Returns `true` when the agent's `allowed_tools`
/// is empty (full surface) OR when it explicitly names a web-shaped
/// tool. Returns `false` when the allow-list is set and excludes web,
/// e.g. `corpus-expert`'s `[corpus_search, corpus_read]`.
///
/// Matching is by substring against a small set of known web tool
/// names — `web_search` and `web_fetch` (leek-side custom). A future
/// `web_*` tool will need to be added here when it ships. We
/// deliberately do not try to parse codex's tool catalog here: the
/// allow-list is the user's contract with the model, and `web_*` is
/// the convention the loader uses to recognize web-tool intent.
pub fn subagent_web_search_allowed(agent: &AgentDef) -> bool {
    // Empty list = full surface — preserve main-agent semantics.
    if agent.allowed_tools.is_empty() {
        return true;
    }
    // Otherwise: any tool whose name contains "web" counts as "allows
    // web access". `web_search` (codex builtin) and `web_fetch` (leek
    // custom) are the two we have today; a `web_pdf_extract` or
    // similar would also match without an update here.
    agent
        .allowed_tools
        .iter()
        .any(|t| t.starts_with("web_") || t == "web_search" || t == "web_fetch")
}

/// M3.6 §E + M3.7 §C + M4.1.3 §P0-3: layer per-subagent overrides
/// from `AgentDef` onto the main agent's `GuardConfig`. Three fields
/// are overridable today:
///
/// - `cost_cap_usd` — a deep-dive worker can carry a $5 budget while
///   a quick-screen worker is locked to $0.20, both independent of
///   the user's main-agent cap. `0.0` is normalized to "no cap" so
///   the spec's "0 = unlimited" product idiom (settings, config file)
///   holds end-to-end. `NaN` / negative are similarly treated as
///   "no cap" — the loader already drops those, but this function
///   is the last line of defense.
/// - `reasoning_effort` — `deep-review` keeps xhigh even after the
///   main agent drops to medium (its isolated context can afford
///   the depth). The loader validates the whitelist; a `None` here
///   means "inherit whatever the main agent currently uses".
/// - `default_max_iterations` (M4.1.3 P0-3) — per-subagent iteration
///   cap. Always wins when present; same posture as `cost_cap_usd`,
///   because the AGENT.md author knows the worker's expected shape
///   better than the main agent's user-tuned cap. The loader already
///   drops `0` / negatives, but the defensive check stays so a future
///   in-memory AgentDef construction does not poison the guard.
/// - `default_max_tool_calls` (M4.1.5) — per-subagent tool-call cap.
///   Mirrors `default_max_iterations` exactly: AGENT.md author knows
///   the worker's tool budget shape; built-ins ship 8 / 12 / 25 / 30 /
///   40 so an over-thinking turn has a hard ceiling.
pub fn apply_agent_overrides(guards: &mut GuardConfig, agent: &AgentDef) {
    if let Some(cap) = agent.cost_cap_usd {
        guards.cost_cap_usd = if cap > 0.0 && cap.is_finite() {
            Some(cap)
        } else {
            None
        };
    }
    if let Some(effort) = agent.reasoning_effort.as_deref() {
        // Loader already validated against the whitelist; defensive
        // double-check guards against a future code path that builds an
        // AgentDef in memory and skips the loader.
        if crate::config::is_valid_reasoning_effort(effort) {
            guards.reasoning_effort = effort.to_string();
        }
    }
    if let Some(iter_cap) = agent.default_max_iterations {
        if iter_cap > 0 {
            guards.max_iterations = Some(iter_cap as usize);
        }
    }
    if let Some(tool_cap) = agent.default_max_tool_calls {
        if tool_cap > 0 {
            guards.max_tool_calls = Some(tool_cap as usize);
        }
    }
}

fn resolve_agent(registry: &Arc<AgentRegistry>, name: &str) -> Result<AgentDef, String> {
    if let Some(a) = registry.get(name) {
        return Ok(a.clone());
    }
    let available = registry.names();
    let hint = if available.is_empty() {
        " (no agents registered)".to_string()
    } else {
        format!(". Available: {}", available.join(", "))
    };
    Err(format!("unknown agent '{name}'{hint}"))
}

/// Suffix appended to the parent turn id to make a unique subagent id.
/// Random 8-char base32 chunk — cheap to generate, room enough that a
/// turn with hundreds of parallel `task` calls won't collide.
fn short_id() -> String {
    use uuid::Uuid;
    let raw = Uuid::new_v4();
    raw.simple().to_string()[..8].to_string()
}

/// Lifecycle frames we emit for the canvas `subagent_card`.
enum LifecyclePhase<'a> {
    Start,
    Completion {
        result_preview: String,
        outcome: &'a loop_core::LoopOutcome,
        agent: &'a AgentDef,
    },
    Error {
        reason: String,
    },
}

#[allow(clippy::too_many_arguments)]
async fn emit_lifecycle(
    st: &AppState,
    session_id: &str,
    parent_turn_id: &str,
    subagent_turn_id: &str,
    agent_name: &str,
    input: &str,
    depth: u32,
    phase: LifecyclePhase<'_>,
) {
    let (phase_str, mut data) = match phase {
        LifecyclePhase::Start => (
            "start",
            serde_json::json!({
                "agent_name": agent_name,
                "input_preview": preview(input),
                "depth": depth,
            }),
        ),
        LifecyclePhase::Completion {
            result_preview,
            outcome,
            agent,
        } => (
            "completion",
            serde_json::json!({
                "agent_name": agent_name,
                "input_preview": preview(input),
                "depth": depth,
                "result_preview": result_preview,
                "stop_reason": outcome.stop_reason,
                "iteration_count": outcome.iteration_count,
                "tool_call_count": outcome.tool_call_count,
                "tool_error_count": outcome.tool_error_count,
                "cost_usd": outcome.cost_usd,
                "wall_clock_ms": outcome.wall_clock_ms,
                "source_layer": agent.source_layer.as_str(),
            }),
        ),
        LifecyclePhase::Error { reason } => (
            "error",
            serde_json::json!({
                "agent_name": agent_name,
                "input_preview": preview(input),
                "depth": depth,
                "error": reason,
            }),
        ),
    };
    // Card identity: one card per spawn. The frontend keys on
    // `subagent_turn_id` so start / completion / error frames merge into
    // the same card.
    data["artifact_id"] = serde_json::Value::String(format!("subagent-{subagent_turn_id}"));
    data["canvas_identity"] =
        serde_json::Value::String(format!("subagent-{subagent_turn_id}"));
    data["subagent_turn_id"] = serde_json::Value::String(subagent_turn_id.to_string());
    data["turn_id"] = serde_json::Value::String(parent_turn_id.to_string());
    data["phase"] = serde_json::Value::String(phase_str.to_string());
    // We don't go through `CanvasArtifact` because the subagent card is
    // its own kind — the existing envelope is tool / search / note only.

    st.emit(session_id, events::kind::SUBAGENT_LIFECYCLE, data).await;
}

/// Compose the digest text the parent agent will read. Same shape as the
/// main agent's `compose_final_text` (we want guard / fatal-error tails
/// to flow back so the parent knows what happened), but keyed off the
/// subagent's own stop_reason.
fn compose_subagent_final_text(
    reply: &str,
    stop_reason: &str,
    fatal: Option<&str>,
) -> String {
    let body = reply.trim();
    if stop_reason == "fatal_error" {
        let err = fatal.unwrap_or("unknown error");
        return if body.is_empty() {
            format!("[subagent fatal_error: {err}]")
        } else {
            format!("{body}\n\n[subagent fatal_error: {err}]")
        };
    }
    let note = match stop_reason {
        "end_turn" => None,
        "max_tokens" => Some("subagent stopped: model reached max_tokens"),
        "idle_timeout" => Some("subagent stopped: idle_timeout"),
        "wall_clock_exceeded" => Some("subagent stopped: wall_clock_exceeded"),
        "max_iterations" => Some("subagent stopped: max_iterations"),
        "cost_cap_exceeded" => Some("subagent stopped: cost_cap_exceeded"),
        // M4.1.5 Task 3: tool-call cap. Parent agent reads the digest +
        // gets to decide whether to re-task with a wider cap.
        "tool_call_cap_exceeded" => Some("subagent stopped: tool_call_cap_exceeded"),
        "doom_loop" => Some("subagent stopped: doom_loop"),
        _ => Some("subagent stopped early"),
    };
    match (body.is_empty(), note) {
        (true, Some(n)) => format!("[{n}]"),
        (true, None) => "（subagent 没有返回任何文本。）".to_string(),
        (false, Some(n)) => format!("{body}\n\n[{n}]"),
        (false, None) => body.to_string(),
    }
}

/// Single-line preview, capped — used for both input + result previews
/// on the subagent_card.
fn preview(s: &str) -> String {
    const MAX: usize = 280;
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= MAX {
        return one;
    }
    let head: String = one.chars().take(MAX).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_returns_body_verbatim_on_end_turn() {
        let out = compose_subagent_final_text("Result body.", "end_turn", None);
        assert_eq!(out, "Result body.");
    }

    #[test]
    fn compose_appends_guard_note() {
        let out = compose_subagent_final_text("partial.", "wall_clock_exceeded", None);
        assert!(out.starts_with("partial."));
        assert!(out.contains("wall_clock_exceeded"));
    }

    #[test]
    fn compose_reports_fatal_with_message() {
        let out = compose_subagent_final_text("", "fatal_error", Some("HTTP 500"));
        assert!(out.contains("HTTP 500"));
        assert!(out.contains("fatal_error"));
    }

    #[test]
    fn compose_handles_empty_natural_end() {
        let out = compose_subagent_final_text("", "end_turn", None);
        assert!(out.contains("没有返回"));
    }

    #[test]
    fn preview_collapses_whitespace_and_caps_length() {
        let long = "a".repeat(1000);
        let p = preview(&long);
        assert!(p.ends_with('…'));
        assert!(p.chars().count() <= 281);
        let multiline = "line one\n  \tline two   line three";
        let p = preview(multiline);
        assert_eq!(p, "line one line two line three");
    }

    fn mk_agent(name: &str, cost_cap_usd: Option<f64>) -> AgentDef {
        AgentDef {
            name: name.into(),
            description: "desc".into(),
            allowed_tools: vec![],
            system_prompt: "body".into(),
            model: None,
            cost_cap_usd,
            reasoning_effort: None,
            default_max_iterations: None,
            default_max_tool_calls: None,
            source_layer: crate::agents::AgentLayer::Builtin,
        }
    }

    fn baseline_guards() -> GuardConfig {
        // Start from a sentinel cap so the override path is observable.
        let mut g = GuardConfig::from_env();
        g.cost_cap_usd = Some(99.0);
        g
    }

    #[test]
    fn agent_override_replaces_main_cost_cap() {
        // M3.6 §E: AGENT.md `cost_cap_usd: 5.0` must replace whatever
        // cap the main agent's `GuardConfig` carries — the subagent
        // gets its own budget, not the user's settings cap.
        let agent = mk_agent("deep-review", Some(5.0));
        let mut guards = baseline_guards();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.cost_cap_usd, Some(5.0));
    }

    #[test]
    fn agent_override_zero_disables_cap() {
        // The product idiom: cost_cap_usd = 0 (or 0.0) means "no cap",
        // mirroring the user-facing settings file. AGENT.md follows
        // the same convention end-to-end.
        let agent = mk_agent("unlimited", Some(0.0));
        let mut guards = baseline_guards();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.cost_cap_usd, None);
    }

    #[test]
    fn agent_override_absent_keeps_main_cap() {
        // `cost_cap_usd` not declared in the frontmatter (None) →
        // subagent inherits whatever the main agent had set.
        let agent = mk_agent("inherit", None);
        let mut guards = baseline_guards();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.cost_cap_usd, Some(99.0), "main cap must survive");
    }

    #[test]
    fn web_search_allowed_for_empty_allowlist() {
        // Empty allow-list = "full surface" semantics. A subagent
        // declared without `allowed_tools` keeps everything the main
        // agent has, including codex builtin web_search.
        let agent = mk_agent("general", None);
        assert!(super::subagent_web_search_allowed(&agent));
    }

    #[test]
    fn web_search_allowed_when_web_tool_listed() {
        // `web_search` or `web_fetch` in the allow-list opts the
        // subagent into web access.
        let mut a = mk_agent("with-search", None);
        a.allowed_tools = vec!["web_search".into(), "corpus_search".into()];
        assert!(super::subagent_web_search_allowed(&a));

        let mut b = mk_agent("with-fetch", None);
        b.allowed_tools = vec!["web_fetch".into()];
        assert!(super::subagent_web_search_allowed(&b));
    }

    #[test]
    fn web_search_blocked_when_allowlist_excludes_web() {
        // The corpus-expert case: allow-list set, no web tool named.
        // The builtin must be turned off so codex doesn't quietly
        // ignore the AGENT.md restriction.
        let mut agent = mk_agent("corpus-expert", None);
        agent.allowed_tools = vec!["corpus_search".into(), "corpus_read".into()];
        assert!(!super::subagent_web_search_allowed(&agent));
    }

    #[test]
    fn web_search_allowed_for_future_web_prefix_tool() {
        // The matcher is `web_*`-prefixed so a future
        // `web_pdf_extract` etc. doesn't need a code change here.
        let mut agent = mk_agent("future", None);
        agent.allowed_tools = vec!["web_pdf_extract".into()];
        assert!(super::subagent_web_search_allowed(&agent));
    }

    #[test]
    fn agent_override_replaces_main_reasoning_effort() {
        // M3.7 §C: AGENT.md `reasoning_effort: xhigh` must swap whatever
        // the main agent's resolver produced — that's how `deep-review`
        // stays at xhigh even after the main agent drops to medium.
        let mut agent = mk_agent("deep-review", None);
        agent.reasoning_effort = Some("xhigh".into());
        let mut guards = baseline_guards();
        // sanity-check the baseline so the override is observable
        guards.reasoning_effort = "medium".into();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.reasoning_effort, "xhigh");
    }

    #[test]
    fn agent_override_absent_keeps_main_reasoning_effort() {
        // No `reasoning_effort` declared → subagent inherits whatever
        // the main agent currently uses (the main loop's resolved value).
        let agent = mk_agent("inherit", None);
        let mut guards = baseline_guards();
        guards.reasoning_effort = "low".into();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.reasoning_effort, "low", "main effort must survive");
    }

    #[test]
    fn agent_override_invalid_reasoning_effort_ignored() {
        // Defensive: the loader strips bad values, but a code path that
        // builds an AgentDef in memory with an unknown effort must not
        // poison the codex request — the override falls through.
        let mut agent = mk_agent("malformed", None);
        agent.reasoning_effort = Some("ultra".into());
        let mut guards = baseline_guards();
        guards.reasoning_effort = "medium".into();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.reasoning_effort, "medium");
    }

    #[test]
    fn agent_override_negative_falls_back_to_no_cap() {
        // Defensive: the loader already strips bad values, but if a
        // future path constructs an AgentDef in code with a negative
        // cap, treat it as "no cap" rather than honoring the bug.
        let agent = mk_agent("negative", Some(-3.0));
        let mut guards = baseline_guards();
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.cost_cap_usd, None);
    }

    #[test]
    fn agent_override_replaces_main_max_iterations() {
        // M4.1.3 (P0-3): AGENT.md `default_max_iterations: 8` replaces
        // whatever cap the main agent's `GuardConfig` carries — the
        // subagent gets its own iteration ceiling. This is the failsafe
        // that prevents a runaway quick-screen from burning through
        // ~50 main-agent iters.
        let mut agent = mk_agent("quick-screen", None);
        agent.default_max_iterations = Some(8);
        let mut guards = baseline_guards();
        // Sanity-check baseline so the override is observable.
        guards.max_iterations = Some(50);
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.max_iterations, Some(8));
    }

    #[test]
    fn agent_override_absent_keeps_main_max_iterations() {
        // No `default_max_iterations` declared (None) → subagent
        // inherits whatever the main agent's resolver produced.
        let agent = mk_agent("inherit-iter", None);
        let mut guards = baseline_guards();
        guards.max_iterations = Some(50);
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.max_iterations, Some(50));
    }

    #[test]
    fn agent_override_zero_iter_cap_ignored() {
        // Defensive: a future in-memory AgentDef constructed with 0
        // should not silently disable the inherited cap. The loader
        // already strips 0, but this is the last line of defense.
        let mut agent = mk_agent("zero-iter", None);
        agent.default_max_iterations = Some(0);
        let mut guards = baseline_guards();
        guards.max_iterations = Some(50);
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.max_iterations, Some(50));
    }

    #[test]
    fn resolve_returns_loaded_agent() {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "alpha".into(),
            AgentDef {
                name: "alpha".into(),
                description: "d".into(),
                allowed_tools: vec![],
                system_prompt: "body".into(),
                model: None,
                cost_cap_usd: None,
                reasoning_effort: None,
                default_max_iterations: None,
                default_max_tool_calls: None,
                source_layer: crate::agents::AgentLayer::Builtin,
            },
        );
        let reg = Arc::new(AgentRegistry::new(m));
        let got = resolve_agent(&reg, "alpha").unwrap();
        assert_eq!(got.name, "alpha");
    }

    #[test]
    fn resolve_lists_available_on_unknown() {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "alpha".into(),
            AgentDef {
                name: "alpha".into(),
                description: "d".into(),
                allowed_tools: vec![],
                system_prompt: "body".into(),
                model: None,
                cost_cap_usd: None,
                reasoning_effort: None,
                default_max_iterations: None,
                default_max_tool_calls: None,
                source_layer: crate::agents::AgentLayer::Builtin,
            },
        );
        let reg = Arc::new(AgentRegistry::new(m));
        let err = resolve_agent(&reg, "ghost").unwrap_err();
        assert!(err.contains("unknown agent 'ghost'"));
        assert!(err.contains("Available: alpha"));
    }

    #[test]
    fn agent_override_replaces_main_max_tool_calls() {
        // M4.1.5: AGENT.md `max_tool_calls: 8` replaces whatever cap
        // the main agent's `GuardConfig` carries — the subagent gets
        // its own tool-call ceiling. Twin of the max_iterations
        // override above.
        let mut agent = mk_agent("quick-screen", None);
        agent.default_max_tool_calls = Some(8);
        let mut guards = baseline_guards();
        guards.max_tool_calls = Some(30);
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.max_tool_calls, Some(8));
    }

    #[test]
    fn agent_override_absent_keeps_main_max_tool_calls() {
        // No `max_tool_calls` declared (None) → subagent inherits
        // whatever the main agent's resolver produced.
        let agent = mk_agent("inherit-tool", None);
        let mut guards = baseline_guards();
        guards.max_tool_calls = Some(30);
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.max_tool_calls, Some(30));
    }

    #[test]
    fn agent_override_zero_tool_cap_ignored() {
        // Defensive: a future in-memory AgentDef constructed with 0
        // should not silently disable the inherited cap. The loader
        // already strips 0, but this is the last line of defense.
        let mut agent = mk_agent("zero-tool", None);
        agent.default_max_tool_calls = Some(0);
        let mut guards = baseline_guards();
        guards.max_tool_calls = Some(30);
        apply_agent_overrides(&mut guards, &agent);
        assert_eq!(guards.max_tool_calls, Some(30));
    }

    #[test]
    fn max_depth_constant_is_two() {
        // Spec §F: depth ≤ 2 enforcement. If this changes, the spec and
        // a depth-bump migration both have to follow.
        assert_eq!(MAX_DEPTH, 2);
    }

    #[tokio::test]
    async fn spawn_refuses_at_depth_cap() {
        // Spec §F: a parent at depth 2 calling `task` (spawn_depth = 3)
        // must short-circuit with a structured error, not actually
        // touch the codex client. We exercise that branch by spawning
        // from a fake parent at depth = MAX_DEPTH (so spawn_depth =
        // MAX_DEPTH + 1 = 3).
        //
        // This test builds a minimal AppState (in-memory SQLite, empty
        // corpus, empty registries) just enough that the dispatcher
        // can construct events. It never reaches `run_loop`, so the
        // codex client is never called — the cap check fires first.
        let st = test_app_state().await;

        let result = spawn(
            &st,
            "test-session",
            "test-turn",
            MAX_DEPTH, // parent already at MAX_DEPTH → spawn depth = 3
            "general-purpose",
            "do a thing",
        )
        .await;

        assert!(result.is_error);
        assert_eq!(result.stop_reason, "max_depth");
        assert!(result.result.contains("Maximum subagent depth"));
        assert_eq!(result.iteration_count, 0);
        assert_eq!(result.cost_usd, 0.0);
    }

    #[tokio::test]
    async fn spawn_errors_on_unknown_agent() {
        // The dispatcher resolves the agent name before doing anything
        // else; an unknown name yields a structured error and a
        // "agent_not_found" stop_reason. No model call happens.
        let st = test_app_state().await;

        let result = spawn(
            &st,
            "test-session",
            "test-turn",
            0, // depth = 0 (main agent), so spawn_depth = 1 — within cap
            "missing-agent",
            "do a thing",
        )
        .await;

        assert!(result.is_error);
        assert_eq!(result.stop_reason, "agent_not_found");
        assert!(result.result.contains("unknown agent"));
        assert!(result.result.contains("missing-agent"));
    }

    /// Build an `AppState` minimal enough to call `spawn` against — we
    /// never run the loop in these tests, but the dispatcher does emit
    /// lifecycle events which need a real event bus + vault pool.
    async fn test_app_state() -> crate::api::AppState {
        use crate::api::AppState;
        use crate::bus::EventBus;
        use crate::config::Config;
        use crate::corpus::Corpus;
        use crate::hooks::HookEngine;
        use crate::llm::codex::CodexClient;
        use crate::skills::SkillRegistry;
        use crate::vault::Vault;
        use crate::vendors::VendorRegistry;
        use std::sync::RwLock;

        let path = std::env::temp_dir()
            .join(format!("leek-subagent-test-{}.db", uuid::Uuid::new_v4()));
        let vault = Vault::open(&path).await.unwrap();
        let codex = CodexClient::new(vault.pool.clone(), crate::vault::LOCAL_USER).unwrap();
        let corpus = Arc::new(Corpus::empty());
        let corpus_graph = Arc::new(corpus.build_graph());
        AppState {
            pool: vault.pool,
            bus: EventBus::new(),
            codex,
            http: reqwest::Client::new(),
            config: Arc::new(RwLock::new(Config::default())),
            web_search: false,
            corpus,
            corpus_graph,
            skills: Arc::new(SkillRegistry::default()),
            hooks: Arc::new(HookEngine::default()),
            agents: Arc::new(AgentRegistry::default()),
            vendors: Arc::new(VendorRegistry::for_test()),
            abort_signals: Arc::new(RwLock::new(std::collections::HashMap::new())),
            codex_sem: Arc::new(tokio::sync::Semaphore::new(crate::api::CODEX_MAX_CONCURRENT)),
        }
    }
}
