//! `use_skill` — load a registered skill's body into the current turn.
//!
//! Returns the skill body as the model's `function_call_output`. The
//! agent loop re-injects every prior `function_call_output` (see
//! `agent::mod`), so the body persists across the rest of the turn just
//! like any other tool result. Canvas renders a `skill_invocation` card
//! (M2.5 research notes §2).
//!
//! `allowed_tools` in the frontmatter is **advisory**: leek surfaces the
//! list in the body header so the model knows the skill's preferred
//! toolset, but does not collapse the tool registry (CC parity — there
//! the field is a pre-approve, not a set-intersection).

use std::sync::Arc;

use serde::Deserialize;

use crate::agent::tools::{summary_snippet, ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::skills::SkillRegistry;

#[derive(Debug, Deserialize)]
struct Args {
    name: String,
}

/// `ToolSpec` for the `use_skill` tool. The description doubles as the
/// system-prompt hint, so it has to read well alongside other tool
/// summaries.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "use_skill".into(),
        description: "Load a registered skill's full guide into this turn. \
                      The skill body — a markdown procedure for a specific \
                      kind of task — becomes available to you immediately, \
                      and stays in the turn's context for subsequent \
                      iterations. Use this when a task matches a skill in \
                      the system-prompt's `# 可用 Skills` index. Pass the \
                      skill's name (the one shown in the index)."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name, exactly as listed in the # 可用 Skills index."
                }
            },
            "required": ["name"],
            "additionalProperties": false,
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "Use skill",
        result: ResultArtifact::Card("skill_invocation"),
        summary: summarize_args,
    }
}

fn summarize_args(args: &serde_json::Value) -> String {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown)");
    summary_snippet(&format!("加载 skill: {name}"))
}

/// Dispatch one `use_skill` call against the in-memory registry.
pub fn run(skills: &Arc<SkillRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let parsed: Args = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return ToolOutcome::error(format!("invalid use_skill arguments: {e}")),
    };

    let Some(skill) = skills.get(&parsed.name) else {
        let available: Vec<&str> = skills
            .iter_sorted()
            .into_iter()
            .map(|s| s.name.as_str())
            .collect();
        let hint = if available.is_empty() {
            String::new()
        } else {
            format!(". Available: {}", available.join(", "))
        };
        return ToolOutcome::error(format!(
            "unknown skill '{}'{hint}",
            parsed.name
        ));
    };

    let header = build_body_header(skill);
    let model_output = format!("{header}\n\n{}", skill.body.as_ref());

    let display_payload = serde_json::json!({
        "skill_name": skill.name,
        "description": skill.description,
        "source_layer": skill.source_layer.as_str(),
        "allowed_tools": skill.allowed_tools,
        // Short body preview so the canvas card can show context without
        // forcing the user to expand the debug view.
        "body_preview": preview_body(&skill.body),
    });
    let debug_payload = serde_json::json!({
        "skill_name": skill.name,
        "body": skill.body.as_ref(),
        "source_layer": skill.source_layer.as_str(),
        "disable_model_invocation": skill.disable_model_invocation,
    });

    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

/// Header prepended to the body so the model sees skill metadata inline
/// (especially `allowed_tools` — advisory but useful as a guidepost).
fn build_body_header(skill: &crate::skills::Skill) -> String {
    let mut s = format!("# Skill: {}\n\n_{}_", skill.name, skill.description);
    if !skill.allowed_tools.is_empty() {
        s.push_str(&format!(
            "\n\n**Recommended tools** (advisory): {}",
            skill.allowed_tools.join(", ")
        ));
    }
    s
}

fn preview_body(body: &str) -> String {
    const MAX: usize = 320;
    let trimmed = body.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillLayer};
    use std::collections::HashMap;

    fn mk_registry(skills: Vec<Skill>) -> Arc<SkillRegistry> {
        let mut map = HashMap::new();
        for s in skills {
            map.insert(s.name.clone(), s);
        }
        Arc::new(SkillRegistry::new(map))
    }

    fn mk_skill(name: &str, body: &str) -> Skill {
        Skill {
            name: name.into(),
            description: format!("desc for {name}"),
            allowed_tools: vec![],
            body: body.into(),
            source_layer: SkillLayer::Builtin,
            disable_model_invocation: false,
        }
    }

    #[test]
    fn spec_is_well_formed() {
        let s = spec();
        assert_eq!(s.name, "use_skill");
        assert!(s.description.contains("skill"));
        assert_eq!(s.parameters["required"], serde_json::json!(["name"]));
    }

    #[test]
    fn run_returns_body_for_known_skill() {
        let skills = mk_registry(vec![mk_skill("corpus-research", "Body content here.")]);
        let outcome = run(
            &skills,
            &serde_json::json!({ "name": "corpus-research" }),
        );
        assert!(!outcome.is_error);
        assert!(outcome.model_output.contains("Body content here."));
        assert!(outcome.model_output.contains("# Skill: corpus-research"));
        assert_eq!(
            outcome.display_payload["skill_name"],
            "corpus-research"
        );
        assert_eq!(
            outcome.display_payload["source_layer"],
            "builtin"
        );
    }

    #[test]
    fn run_emits_advisory_tools_in_body_header() {
        let mut s = mk_skill("with-tools", "Body.");
        s.allowed_tools = vec!["web_fetch".into(), "corpus_search".into()];
        let skills = mk_registry(vec![s]);
        let outcome = run(&skills, &serde_json::json!({ "name": "with-tools" }));
        assert!(outcome.model_output.contains("Recommended tools"));
        assert!(outcome.model_output.contains("web_fetch"));
        assert!(outcome.model_output.contains("corpus_search"));
    }

    #[test]
    fn run_errors_on_unknown_skill() {
        let skills = mk_registry(vec![mk_skill("alpha", "body")]);
        let outcome = run(&skills, &serde_json::json!({ "name": "missing" }));
        assert!(outcome.is_error);
        assert!(outcome.model_output.contains("unknown skill"));
        // Hint lists available skills so the model can recover.
        assert!(outcome.model_output.contains("alpha"));
    }

    #[test]
    fn run_errors_on_malformed_args() {
        let skills = mk_registry(vec![]);
        let outcome = run(&skills, &serde_json::json!({}));
        assert!(outcome.is_error);
        assert!(outcome.model_output.contains("invalid"));
    }

    #[test]
    fn summary_includes_skill_name() {
        let summary = summarize_args(&serde_json::json!({ "name": "deep-dive" }));
        assert!(summary.contains("deep-dive"));
    }

    #[test]
    fn body_preview_truncates_long_body() {
        let long = "x".repeat(1000);
        let p = preview_body(&long);
        assert!(p.ends_with('…'));
        assert!(p.chars().count() <= 321);
    }
}
