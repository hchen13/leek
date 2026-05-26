//! Client-side function tools — the M1.9 tool set.
//!
//! M1 kept the surface tiny (`web_fetch`). M1.9 adds `update_plan` and
//! splits two concerns the model and the workbench need kept apart:
//!
//! - The **LLM-facing spec** (`ToolSpec`: name, description, JSON schema) is
//!   all the model sees.
//! - The **UI metadata** (`ToolUi`: display name, where the result renders,
//!   a one-line `summary`) is for the workbench only and never enters the
//!   model's context (REQUIREMENTS §4.1).
//!
//! A run returns three surfaces (REQUIREMENTS §4.2), produced by one
//! execution: `model_output` goes to the model via `function_call_output`;
//! `display_payload` is the structured body of the canvas card;
//! `debug_payload` backs its expand / debug view. The agent loop never
//! transforms business data — the tool produces all three, the loop only
//! plumbs them. Tool names and descriptions stay vendor-neutral
//! (ARCHITECTURE §12.1).

mod corpus_read;
mod corpus_search;
mod get_announcements;
mod get_business_breakdown;
mod get_candlesticks;
mod get_capital_flow;
mod get_company_info;
mod get_concepts;
mod get_consensus;
mod get_financials;
mod get_industry_peers;
mod get_top_holders;
mod market_quote;
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
    /// The model's view of the result — goes into `function_call_output`.
    pub model_output: String,
    /// Structured body of the canvas card. UI-only; never sent to the model.
    pub display_payload: serde_json::Value,
    /// Detail for the card's expand / debug view.
    pub debug_payload: serde_json::Value,
    pub is_error: bool,
}

impl ToolOutcome {
    /// A successful run, with the three result surfaces given explicitly.
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

    /// A failed run. The message is the same across all three surfaces — an
    /// error is not business data, so there is nothing to keep apart: the
    /// model sees it to recover, the card shows it.
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
    /// A canvas tool card (REQUIREMENTS §2.4). The string is the renderer
    /// hint (`card_kind`).
    Card(&'static str),
    /// The right-rail Plan / TODO widget — not a canvas card (§2.6).
    Plan,
}

/// UI-only metadata for a tool (REQUIREMENTS §4.1). Registered alongside the
/// `ToolSpec`, but kept strictly separate: none of this reaches the model.
#[derive(Clone, Copy)]
pub struct ToolUi {
    /// Human-readable name for cards and the chat tool summary.
    pub display_name: &'static str,
    /// Where this tool's result renders.
    pub result: ResultArtifact,
    /// A one-line human summary of a call, derived from its arguments
    /// (REQUIREMENTS §2.1's running tool summary). The frontend must not
    /// summarize tool args itself (§8.2), so the backend supplies this.
    pub summary: fn(&serde_json::Value) -> String,
}

/// The function-tool specs offered to the model this milestone.
///
/// `use_skill` is conditional on a non-empty skill registry — exposing
/// it with nothing to load would invite a guaranteed-error call.
/// `task` is always offered: even when no AGENT.md is registered, the
/// task tool can fall back to `general-purpose` (the built-in baseline),
/// so the surface stays stable. The dispatch path still errors loudly
/// if the named agent is genuinely absent — we don't want the surface
/// to silently disappear because a user removed an agent file.
pub fn specs(skills: &Arc<SkillRegistry>, agents: &Arc<AgentRegistry>) -> Vec<ToolSpec> {
    let mut v = vec![
        web_fetch::spec(),
        update_plan::spec(),
        corpus_search::spec(),
        corpus_read::spec(),
        // M3 — A-share research tools. Always offered; an absent
        // upstream token surfaces as a per-call structured error
        // rather than removing the surface (the model can prompt
        // the user to configure one).
        market_quote::spec(),
        get_candlesticks::spec(),
        get_financials::spec(),
        get_company_info::spec(),
        get_capital_flow::spec(),
        // M4.1 — supplementary A-share research tools.
        get_industry_peers::spec(),
        get_business_breakdown::spec(),
        get_announcements::spec(),
        get_consensus::spec(),
        get_top_holders::spec(),
        get_concepts::spec(),
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
        "market_quote" => Some(market_quote::ui()),
        "get_candlesticks" => Some(get_candlesticks::ui()),
        "get_financials" => Some(get_financials::ui()),
        "get_company_info" => Some(get_company_info::ui()),
        "get_capital_flow" => Some(get_capital_flow::ui()),
        // M4.1
        "get_industry_peers" => Some(get_industry_peers::ui()),
        "get_business_breakdown" => Some(get_business_breakdown::ui()),
        "get_announcements" => Some(get_announcements::ui()),
        "get_consensus" => Some(get_consensus::ui()),
        "get_top_holders" => Some(get_top_holders::ui()),
        "get_concepts" => Some(get_concepts::ui()),
        _ => None,
    }
}

/// Per-call context the dispatch path needs, beyond the (skills /
/// corpus / http) values that have been there since M2.1.
///
/// `task` (M2.7) is the one tool that needs the live `AppState`, the
/// caller's session / turn ids, and the depth-cap context to spawn a
/// subagent loop. The other tools ignore everything in this struct. We
/// pass it explicitly so unit tests that don't exercise `task` can
/// build a minimal struct without a full `AppState`.
pub struct DispatchCtx<'a> {
    /// `Some` enables `task`; `None` makes a `task` call error out with
    /// "task not available in this dispatch context". Tests that don't
    /// exercise `task` pass `None`.
    pub st: Option<&'a AppState>,
    pub session_id: &'a str,
    pub parent_turn_id: &'a str,
    /// Depth of the calling turn (main agent = 0, subagent = 1, …).
    pub parent_depth: u32,
}

impl<'a> DispatchCtx<'a> {
    /// Convenience: a context that disables `task` (for unit tests of
    /// the other tools).
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

/// Dispatch one function call. An unknown name is a structured error, not a
/// panic — the model gets it back as `function_call_output` and can recover.
///
/// 8 explicit `&Arc<…>` registries threaded through one call site is a
/// lot, but each one is a different runtime surface (HTTP / corpus /
/// skills / agents / vendors). Bundling them into a "ServiceBag" struct
/// would make the call-site shorter at the cost of obscuring the
/// dependency surface; given the small number of dispatch sites (loop +
/// two unit tests) we accept the parameter count.
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
        // M3 — A-share research tools. Each runs the vendor-fallback
        // chain inside its own module; the loop sees only the
        // `ToolOutcome` triple.
        "market_quote" => market_quote::run(vendors, args).await,
        "get_candlesticks" => get_candlesticks::run(vendors, args).await,
        "get_financials" => get_financials::run(vendors, args).await,
        "get_company_info" => get_company_info::run(vendors, args).await,
        "get_capital_flow" => get_capital_flow::run(vendors, args).await,
        // M4.1 — supplementary A-share research tools.
        "get_industry_peers" => get_industry_peers::run(vendors, args).await,
        "get_business_breakdown" => get_business_breakdown::run(vendors, args).await,
        "get_announcements" => get_announcements::run(vendors, args).await,
        "get_consensus" => get_consensus::run(vendors, args).await,
        "get_top_holders" => get_top_holders::run(vendors, args).await,
        "get_concepts" => get_concepts::run(vendors, args).await,
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
                "task: tool not available in this dispatch context (no AppState bound)".to_string(),
            ),
        },
        other => ToolOutcome::error(format!(
            "unknown tool '{other}' — it is not in the available tool set"
        )),
    }
}

/// Build the canvas-artifact frame for a tool call (REQUIREMENTS §2.2, §2.4).
/// `outcome = None` is the `Start` frame, emitted before dispatch; `Some` is
/// the `Completion` / `Error` frame. UI metadata comes from the registry and
/// the display / debug payloads from the tool — the loop never transforms
/// business data (REQUIREMENTS §4).
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
        // Spec §A: the task tool is always advertised — the
        // default fallback agent `general-purpose` is built-in,
        // so the surface stays stable across boots.
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
        assert!(ui("web_fetch").is_some());
        assert!(ui("update_plan").is_some());
        assert!(ui("corpus_search").is_some());
        assert!(ui("corpus_read").is_some());
        assert!(ui("task").is_some());
        // M3 — A-share research tools.
        assert!(ui("market_quote").is_some());
        assert!(ui("get_candlesticks").is_some());
        assert!(ui("get_financials").is_some());
        assert!(ui("get_company_info").is_some());
        assert!(ui("get_capital_flow").is_some());
        // M4.1 — supplementary A-share research tools.
        assert!(ui("get_industry_peers").is_some());
        assert!(ui("get_business_breakdown").is_some());
        assert!(ui("get_announcements").is_some());
        assert!(ui("get_consensus").is_some());
        assert!(ui("get_top_holders").is_some());
        assert!(ui("get_concepts").is_some());
        assert!(ui("nope").is_none());
        // update_plan renders to the right rail, not a canvas tool card.
        assert_eq!(ui("update_plan").unwrap().result, ResultArtifact::Plan);
        // corpus tools render as canvas cards with their own kinds.
        assert_eq!(
            ui("corpus_search").unwrap().result,
            ResultArtifact::Card("corpus_search")
        );
        assert_eq!(
            ui("corpus_read").unwrap().result,
            ResultArtifact::Card("corpus_read")
        );
        // task tool has a placeholder canvas kind — actual rendering is
        // the subagent_card emitted via the lifecycle event.
        assert_eq!(
            ui("task").unwrap().result,
            ResultArtifact::Card("task_dispatch")
        );
    }

    #[test]
    fn dispatch_task_without_appstate_returns_error_not_panic() {
        // Unit tests can call task with `DispatchCtx::for_test()` —
        // expecting a structured error, not a crash.
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

    #[test]
    fn tool_artifact_start_then_done_share_one_card() {
        let args = serde_json::json!({ "url": "https://example.com" });
        let start = tool_artifact("t", 1, "c1", "web_fetch", &args, None).into_payload();
        assert_eq!(start["phase"], "start");
        assert_eq!(start["data"]["tool"], "web_fetch");
        assert!(start["data"]["display_name"].is_string());
        assert!(start["data"]["summary"].is_string());
        // No result payload on the start frame.
        assert!(start["data"]["display_payload"].is_null());

        let outcome = ToolOutcome::ok(
            "ok",
            serde_json::json!({ "url": "https://example.com" }),
            serde_json::json!({}),
        );
        let done = tool_artifact("t", 1, "c1", "web_fetch", &args, Some(&outcome)).into_payload();
        assert_eq!(done["phase"], "completion");
        assert_eq!(done["data"]["display_payload"]["url"], "https://example.com");
        // Same id and identity → the frontend updates one card, not two.
        assert_eq!(start["artifact_id"], done["artifact_id"]);
        assert_eq!(start["canvas_identity"], done["canvas_identity"]);
    }
}
