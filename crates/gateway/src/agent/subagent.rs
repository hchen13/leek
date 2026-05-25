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

    let guards = GuardConfig::resolve(&st.config_snapshot());

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
                source_layer: crate::agents::AgentLayer::Builtin,
            },
        );
        let reg = Arc::new(AgentRegistry::new(m));
        let err = resolve_agent(&reg, "ghost").unwrap_err();
        assert!(err.contains("unknown agent 'ghost'"));
        assert!(err.contains("Available: alpha"));
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
        }
    }
}
