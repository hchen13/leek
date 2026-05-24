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
mod update_plan;
mod use_skill;
mod web_fetch;

use std::sync::Arc;

use crate::agent::events::{self, CanvasArtifact, Phase};
use crate::corpus::Corpus;
use crate::llm::ToolSpec;
use crate::skills::SkillRegistry;

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

/// The function-tool specs offered to the model this milestone. `use_skill`
/// is conditional on a non-empty registry — exposing the tool when no skills
/// are registered would invite the model to call it and fail.
pub fn specs(skills: &Arc<SkillRegistry>) -> Vec<ToolSpec> {
    let mut v = vec![
        web_fetch::spec(),
        update_plan::spec(),
        corpus_search::spec(),
        corpus_read::spec(),
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
        _ => None,
    }
}

/// Dispatch one function call. An unknown name is a structured error, not a
/// panic — the model gets it back as `function_call_output` and can recover.
///
/// The `corpus` arg is the shared read-only knowledge layer (M2.1); only
/// the corpus tools consume it, but it lives on the dispatch signature
/// rather than a per-tool closure so the agent loop has one call site.
pub async fn dispatch(
    http: &reqwest::Client,
    corpus: &Arc<Corpus>,
    skills: &Arc<SkillRegistry>,
    name: &str,
    args: &serde_json::Value,
) -> ToolOutcome {
    match name {
        "web_fetch" => web_fetch::run(http, args).await,
        "update_plan" => update_plan::run(args),
        "corpus_search" => corpus_search::run(corpus, args),
        "corpus_read" => corpus_read::run(corpus, args),
        "use_skill" => use_skill::run(skills, args),
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
        let out = futures::executor::block_on(dispatch(
            &http,
            &corpus,
            &skills,
            "nope",
            &serde_json::Value::Null,
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("unknown tool"));
    }

    #[test]
    fn use_skill_tool_is_only_advertised_when_registry_non_empty() {
        let empty = Arc::new(SkillRegistry::default());
        let names: Vec<_> = specs(&empty).into_iter().map(|s| s.name).collect();
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
        let names: Vec<_> = specs(&with_one).into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"use_skill".to_string()));
        assert!(ui("use_skill").is_some());
    }

    #[test]
    fn ui_registry_covers_the_tool_set() {
        assert!(ui("web_fetch").is_some());
        assert!(ui("update_plan").is_some());
        assert!(ui("corpus_search").is_some());
        assert!(ui("corpus_read").is_some());
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
