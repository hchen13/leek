//! Function tools — M4.1.1 facts-only A-share kit.
//!
//! Each tool returns a `ToolOutcome` triple (REQUIREMENTS §4.2):
//!
//! - `model_output` — distilled markdown ≤ 1500 tokens default
//!   (≤ 4000 in focus modes). Carries load-bearing facts only.
//! - `display_payload` — raw structured data for the canvas card.
//!   Vendor JSON is normalized into typed shapes from `vendors/types.rs`.
//! - `debug_payload` — debug-view detail (vendor identity, attempts,
//!   timing).
//!
//! The tools fan out internally with `tokio::try_join!`, talk to one
//! or more vendor methods, then surface partial gaps as
//! `empty_dimensions` (an array of focus / dimension keys that came
//! back empty) inside `display_payload` — **never** a tool-level error.
//! That keeps the model honest: "this section is unavailable" surfaces
//! as a markdown line ("数据来源: 暂不可用") and a structured
//! `empty_dimensions` flag the model can read, instead of triggering a
//! pointless retry.

mod chart_data;
mod corpus_read;
mod corpus_search;
mod industry_landscape;
mod macro_indicators;
mod market_overview;
mod market_pulse;
mod read_pdf;
mod recent_actions;
mod research_sentiment;
mod stock_overview;
mod task;
mod update_plan;
mod use_skill;
mod web_fetch;

use std::sync::Arc;

use crate::agent::events::{self, CanvasArtifact, Phase};
use crate::agents::AgentRegistry;
use crate::api::AppState;
use crate::corpus::Corpus;
use crate::llm::ToolSpec;
use crate::skills::SkillRegistry;
use crate::vendors::VendorRegistry;

/// Outcome of running one tool call — the three result surfaces of
/// REQUIREMENTS §4.2, produced by a single execution.
pub struct ToolOutcome {
    /// The model's view — goes into `function_call_output`.
    pub model_output: String,
    /// Structured body of the canvas card. UI-only; never sent to the
    /// model. `empty_dimensions: [..]` lives inside `display_payload`.
    pub display_payload: serde_json::Value,
    /// Debug-view detail (vendor identity, attempts, timing).
    pub debug_payload: serde_json::Value,
    pub is_error: bool,
}

impl ToolOutcome {
    pub(super) fn ok(
        model_output: impl Into<String>,
        display_payload: serde_json::Value,
        debug_payload: serde_json::Value,
    ) -> Self {
        Self {
            model_output: model_output.into(),
            display_payload,
            debug_payload,
            is_error: false,
        }
    }

    pub(super) fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            display_payload: serde_json::json!({ "error": message.clone() }),
            debug_payload: serde_json::json!({ "error": message.clone() }),
            model_output: message,
            is_error: true,
        }
    }
}

/// Where a tool's result renders on the workbench.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResultArtifact {
    Card(&'static str),
    Plan,
}

/// UI-only metadata.
#[derive(Clone, Copy)]
pub struct ToolUi {
    pub display_name: &'static str,
    pub result: ResultArtifact,
    pub summary: fn(&serde_json::Value) -> String,
}

/// Specs offered to the model this milestone. `use_skill` only when
/// at least one skill is loaded; `task` is always offered (has a
/// built-in `general-purpose` fallback).
pub fn specs(skills: &Arc<SkillRegistry>, agents: &Arc<AgentRegistry>) -> Vec<ToolSpec> {
    let mut v = vec![
        web_fetch::spec(),
        update_plan::spec(),
        corpus_search::spec(),
        corpus_read::spec(),
        // M4.1.1 facts-only A-share research kit.
        macro_indicators::spec(),
        industry_landscape::spec(),
        market_overview::spec(),
        stock_overview::spec(),
        recent_actions::spec(),
        market_pulse::spec(),
        research_sentiment::spec(),
        chart_data::spec(),
        read_pdf::spec(),
        task::spec(agents),
    ];
    if !skills.is_empty() {
        v.push(use_skill::spec());
    }
    v
}

/// UI metadata for a tool, or `None` for an unknown name.
pub fn ui(name: &str) -> Option<ToolUi> {
    match name {
        "web_fetch" => Some(web_fetch::ui()),
        "update_plan" => Some(update_plan::ui()),
        "corpus_search" => Some(corpus_search::ui()),
        "corpus_read" => Some(corpus_read::ui()),
        "use_skill" => Some(use_skill::ui()),
        "task" => Some(task::ui()),
        // M4.1.1
        "macro_indicators" => Some(macro_indicators::ui()),
        "industry_landscape" => Some(industry_landscape::ui()),
        "market_overview" => Some(market_overview::ui()),
        "stock_overview" => Some(stock_overview::ui()),
        "recent_actions" => Some(recent_actions::ui()),
        "market_pulse" => Some(market_pulse::ui()),
        "research_sentiment" => Some(research_sentiment::ui()),
        "chart_data" => Some(chart_data::ui()),
        "read_pdf" => Some(read_pdf::ui()),
        _ => None,
    }
}

/// Per-call context the dispatch path needs beyond the (skills /
/// corpus / http) values that have been there since M2.1.
pub struct DispatchCtx<'a> {
    pub st: Option<&'a AppState>,
    pub session_id: &'a str,
    pub parent_turn_id: &'a str,
    pub parent_depth: u32,
}

impl<'a> DispatchCtx<'a> {
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            st: None,
            session_id: "",
            parent_turn_id: "",
            parent_depth: 0,
        }
    }
}

/// Dispatch one function call.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch(
    ctx: &DispatchCtx<'_>,
    http: &reqwest::Client,
    corpus: &Arc<Corpus>,
    skills: &Arc<SkillRegistry>,
    _agents: &Arc<AgentRegistry>,
    vendors: &Arc<VendorRegistry>,
    name: &str,
    args: &serde_json::Value,
) -> ToolOutcome {
    match name {
        "web_fetch" => web_fetch::run(http, args).await,
        "update_plan" => update_plan::run(args),
        "corpus_search" => corpus_search::run(corpus, args),
        "corpus_read" => corpus_read::run(corpus, args),
        "use_skill" => use_skill::run(skills, args),
        // M4.1.1 facts-only A-share kit.
        "macro_indicators" => macro_indicators::run(vendors, args).await,
        "industry_landscape" => industry_landscape::run(vendors, args).await,
        "market_overview" => market_overview::run(vendors, args).await,
        "stock_overview" => stock_overview::run(vendors, args).await,
        "recent_actions" => recent_actions::run(vendors, args).await,
        "market_pulse" => market_pulse::run(vendors, args).await,
        "research_sentiment" => research_sentiment::run(vendors, args).await,
        "chart_data" => chart_data::run(vendors, args).await,
        "read_pdf" => read_pdf::run(http, args).await,
        "task" => match ctx.st {
            Some(st) => {
                task::run(
                    st,
                    ctx.session_id,
                    ctx.parent_turn_id,
                    ctx.parent_depth,
                    args,
                )
                .await
            }
            None => ToolOutcome::error(
                "task: tool not available in this dispatch context (no AppState bound)"
                    .to_string(),
            ),
        },
        other => ToolOutcome::error(format!(
            "unknown tool '{other}' — it is not in the available tool set"
        )),
    }
}

/// Build the canvas-artifact frame for a tool call.
pub fn tool_artifact(
    turn_id: &str,
    iteration: usize,
    call_id: &str,
    name: &str,
    args: &serde_json::Value,
    outcome: Option<&ToolOutcome>,
) -> CanvasArtifact {
    let registered = ui(name);
    let display_name = registered.map(|u| u.display_name).unwrap_or(name);
    let card_kind = match registered.map(|u| u.result) {
        Some(ResultArtifact::Card(kind)) => kind,
        _ => "tool",
    };
    let summary = registered
        .map(|u| (u.summary)(args))
        .unwrap_or_else(|| name.to_string());

    let (phase, mut data) = match outcome {
        None => (Phase::Start, serde_json::json!({})),
        Some(o) => (
            if o.is_error {
                Phase::Error
            } else {
                Phase::Completion
            },
            serde_json::json!({
                "display_payload": o.display_payload,
                "debug_payload": o.debug_payload,
            }),
        ),
    };
    data["tool"] = name.into();
    data["call_id"] = call_id.into();
    data["display_name"] = display_name.into();
    data["card_kind"] = card_kind.into();
    data["summary"] = summary.into();
    data["arguments"] = args.clone();

    CanvasArtifact::tool(
        turn_id,
        iteration,
        call_id,
        events::default_canvas_identity(name, args),
        phase,
        data,
    )
}

/// Trim a value to a short single-line snippet for a tool `summary`.
pub(super) fn summary_snippet(s: &str) -> String {
    const MAX: usize = 40;
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        format!("{}…", one_line.chars().take(MAX).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_unknown_tool_is_a_structured_error() {
        let http = reqwest::Client::new();
        let corpus = Arc::new(Corpus::empty());
        let skills = Arc::new(SkillRegistry::default());
        let agents = Arc::new(AgentRegistry::default());
        let vendors = Arc::new(VendorRegistry::for_test());
        let ctx = DispatchCtx::for_test();
        let out = futures::executor::block_on(dispatch(
            &ctx,
            &http,
            &corpus,
            &skills,
            &agents,
            &vendors,
            "nope",
            &serde_json::Value::Null,
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("unknown tool"));
    }

    #[test]
    fn use_skill_tool_is_only_advertised_when_registry_non_empty() {
        let empty = Arc::new(SkillRegistry::default());
        let agents = Arc::new(AgentRegistry::default());
        let names: Vec<_> = specs(&empty, &agents).into_iter().map(|s| s.name).collect();
        assert!(!names.contains(&"use_skill".to_string()));

        let mut map = std::collections::HashMap::new();
        map.insert(
            "alpha".to_string(),
            crate::skills::Skill {
                name: "alpha".into(),
                description: "d".into(),
                allowed_tools: vec![],
                body: "b".into(),
                source_layer: crate::skills::SkillLayer::Builtin,
                disable_model_invocation: false,
            },
        );
        let with_one = Arc::new(SkillRegistry::new(map));
        let names: Vec<_> = specs(&with_one, &agents)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(names.contains(&"use_skill".to_string()));
        assert!(ui("use_skill").is_some());
    }

    #[test]
    fn task_tool_is_always_offered() {
        let empty_skills = Arc::new(SkillRegistry::default());
        let empty_agents = Arc::new(AgentRegistry::default());
        let names: Vec<_> = specs(&empty_skills, &empty_agents)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(names.contains(&"task".to_string()));
        assert!(ui("task").is_some());
    }

    #[test]
    fn ui_registry_covers_the_tool_set() {
        for n in [
            "web_fetch",
            "update_plan",
            "corpus_search",
            "corpus_read",
            "task",
            "macro_indicators",
            "industry_landscape",
            "market_overview",
            "stock_overview",
            "recent_actions",
            "market_pulse",
            "research_sentiment",
            "chart_data",
            "read_pdf",
        ] {
            assert!(ui(n).is_some(), "missing UI registration for {n}");
        }
        assert!(ui("nope").is_none());
        assert_eq!(ui("update_plan").unwrap().result, ResultArtifact::Plan);
    }

    #[test]
    fn dispatch_task_without_appstate_returns_error_not_panic() {
        let http = reqwest::Client::new();
        let corpus = Arc::new(Corpus::empty());
        let skills = Arc::new(SkillRegistry::default());
        let agents = Arc::new(AgentRegistry::default());
        let vendors = Arc::new(VendorRegistry::for_test());
        let ctx = DispatchCtx::for_test();
        let out = futures::executor::block_on(dispatch(
            &ctx,
            &http,
            &corpus,
            &skills,
            &agents,
            &vendors,
            "task",
            &serde_json::json!({ "input": "hi" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("not available"));
    }
}
