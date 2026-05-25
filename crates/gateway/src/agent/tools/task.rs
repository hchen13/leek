//! `task` — delegate a subtask to a registered subagent (M2.7).
//!
//! Aligned with Claude Code's `Task` tool:
//!
//! ```text
//! task(agent_name?: string, input: string) → { result, subagent_turn_id, ... }
//! ```
//!
//! The dispatch path resolves `agent_name` against `AgentRegistry`,
//! spawns a child agent loop via `agent::subagent::spawn`, and returns
//! the subagent's final text as the tool's `model_output`. The card's
//! `display_payload` carries the lifecycle roll-up (cost, wall-clock,
//! iteration count, stop_reason) so the canvas `subagent_card` can show
//! it without re-querying.

use std::sync::Arc;

use serde::Deserialize;

use crate::agent::subagent;
use crate::agent::tools::{summary_snippet, ResultArtifact, ToolOutcome, ToolUi};
use crate::agents::AgentRegistry;
use crate::api::AppState;
use crate::llm::ToolSpec;

/// Default agent name when the model omits `agent_name`. Aligned with
/// CC's "general-purpose" baseline (spec §H) — always present in the
/// shipped registry, with the full tool surface.
const DEFAULT_AGENT: &str = "general-purpose";

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    agent_name: Option<String>,
    input: String,
}

/// `ToolSpec` for the `task` tool. The description enumerates the
/// registered agent names so the model can pick the right one without
/// a separate registry-listing tool.
pub fn spec(agents: &Arc<AgentRegistry>) -> ToolSpec {
    let roster = registered_roster(agents);
    let description = format!(
        "Delegate a self-contained sub-task to a named subagent. The subagent runs \
         its own agent loop with a focused system prompt and (optionally) a restricted \
         tool surface, and returns a text digest of its findings. Use this for tasks that \
         would otherwise bloat the main turn's context (deep corpus dives, multi-page web \
         walks, parallelizable per-ticker analysis). The subagent has NO memory across \
         calls — each `task` is a fresh loop. The subagent CAN itself call `task` once \
         (depth 2 max). Available agents: {roster}."
    );
    ToolSpec {
        name: "task".into(),
        description,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_name": {
                    "type": "string",
                    "description": format!(
                        "Subagent name from the registry. Defaults to '{DEFAULT_AGENT}' \
                         (full tool surface) when omitted."
                    )
                },
                "input": {
                    "type": "string",
                    "description": "The task you want the subagent to do. A standalone \
                                    prompt — the subagent has no other context."
                }
            },
            "required": ["input"],
            "additionalProperties": false,
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "Delegate (task)",
        // The `task` tool intentionally has no canvas tool card — the
        // subagent's whole activity lives in its own `subagent_card`
        // emitted by `agent::subagent::spawn`. We still register a kind
        // here so the inner loop's tool dispatcher knows where the tool
        // belongs, but the loop suppresses the redundant tool card emit
        // by routing through the lifecycle event instead.
        result: ResultArtifact::Card("task_dispatch"),
        summary: summarize_args,
    }
}

fn summarize_args(args: &serde_json::Value) -> String {
    let agent = args
        .get("agent_name")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_AGENT);
    let input = args
        .get("input")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    summary_snippet(&format!("→ {agent}: {input}"))
}

/// Spawn the subagent loop and turn its result into a `ToolOutcome`.
/// `parent_depth` is the depth of the turn that called `task`; the
/// spawn site bumps it by one and refuses at depth > 2.
pub async fn run(
    st: &AppState,
    session_id: &str,
    parent_turn_id: &str,
    parent_depth: u32,
    args: &serde_json::Value,
) -> ToolOutcome {
    let parsed: Args = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return ToolOutcome::error(format!("invalid task arguments: {e}")),
    };
    if parsed.input.trim().is_empty() {
        return ToolOutcome::error("task: 'input' must be non-empty.");
    }
    let agent_name = parsed
        .agent_name
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_AGENT);

    let result = subagent::spawn(
        st,
        session_id,
        parent_turn_id,
        parent_depth,
        agent_name,
        &parsed.input,
    )
    .await;

    let model_output = format!(
        "## Subagent result ({}, turn {})\n\n{}\n\n\
         _stop_reason: {}, iterations: {}, tool_calls: {}, cost: ${:.4}, wall: {}ms_",
        result.agent_name,
        result.subagent_turn_id,
        result.result,
        result.stop_reason,
        result.iteration_count,
        result.tool_call_count,
        result.cost_usd,
        result.wall_clock_ms,
    );

    let display_payload = serde_json::json!({
        "agent_name": result.agent_name,
        "subagent_turn_id": result.subagent_turn_id,
        "stop_reason": result.stop_reason,
        "iteration_count": result.iteration_count,
        "tool_call_count": result.tool_call_count,
        "cost_usd": result.cost_usd,
        "wall_clock_ms": result.wall_clock_ms,
    });
    let debug_payload = serde_json::json!({
        "agent_name": result.agent_name,
        "subagent_turn_id": result.subagent_turn_id,
        "stop_reason": result.stop_reason,
        "result": result.result,
        "is_error": result.is_error,
    });

    if result.is_error {
        ToolOutcome {
            model_output,
            display_payload,
            debug_payload,
            is_error: true,
        }
    } else {
        ToolOutcome::ok(model_output, display_payload, debug_payload)
    }
}

/// Comma-separated list of registered agent names plus their
/// descriptions. Empty registry → a single "general-purpose (built-in)"
/// pointer so the model still has a sensible default.
fn registered_roster(agents: &Arc<AgentRegistry>) -> String {
    if agents.is_empty() {
        return format!("{DEFAULT_AGENT} (built-in default)");
    }
    agents
        .iter_sorted()
        .iter()
        .map(|a| format!("`{}` ({})", a.name, a.description))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentLayer, AgentRegistry};
    use std::collections::HashMap;

    fn mk_registry() -> Arc<AgentRegistry> {
        let mut m = HashMap::new();
        m.insert(
            DEFAULT_AGENT.to_string(),
            AgentDef {
                name: DEFAULT_AGENT.into(),
                description: "通用 worker".into(),
                allowed_tools: vec![],
                system_prompt: "body".into(),
                model: None,
                cost_cap_usd: None,
                source_layer: AgentLayer::Builtin,
            },
        );
        m.insert(
            "corpus-expert".into(),
            AgentDef {
                name: "corpus-expert".into(),
                description: "corpus 专家".into(),
                allowed_tools: vec!["corpus_search".into(), "corpus_read".into()],
                system_prompt: "body".into(),
                model: None,
                cost_cap_usd: None,
                source_layer: AgentLayer::Builtin,
            },
        );
        Arc::new(AgentRegistry::new(m))
    }

    #[test]
    fn spec_is_well_formed() {
        let reg = mk_registry();
        let s = spec(&reg);
        assert_eq!(s.name, "task");
        assert!(s.description.contains("subagent"));
        // The roster names land in the description so the model can pick
        // without needing a separate listing call.
        assert!(s.description.contains("general-purpose"));
        assert!(s.description.contains("corpus-expert"));
        assert_eq!(s.parameters["required"], serde_json::json!(["input"]));
    }

    #[test]
    fn empty_registry_still_yields_a_roster() {
        let reg = Arc::new(AgentRegistry::default());
        let s = spec(&reg);
        // No agents loaded — the default name is still surfaced so the
        // tool is callable in principle, but a real call still errors
        // out at dispatch ("unknown agent"). This is the right tradeoff:
        // stable surface, loud failure.
        assert!(s.description.contains("general-purpose"));
    }

    #[test]
    fn summary_includes_agent_name_and_input() {
        let s = summarize_args(&serde_json::json!({
            "agent_name": "corpus-expert",
            "input": "查询 circle of competence",
        }));
        assert!(s.contains("corpus-expert"));
    }

    #[test]
    fn summary_uses_default_when_agent_omitted() {
        let s = summarize_args(&serde_json::json!({ "input": "do a thing" }));
        assert!(s.contains(DEFAULT_AGENT));
    }
}
