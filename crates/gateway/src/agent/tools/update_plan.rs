//! `update_plan` — maintain the user-visible Plan / TODO list.
//!
//! A client-side tool like the others, but its result renders to the
//! right-rail Plan widget, not a canvas tool card (REQUIREMENTS §2.6): the
//! agent loop routes it by its `ResultArtifact::Plan` UI metadata. The plan
//! is display-only — it never gates the turn; the model may finish whenever
//! the answer is ready. Each call replaces the whole plan, so there is no
//! plan state to persist (REQUIREMENTS §6): the `plan_updated` event log is
//! the record.

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;

const STATUSES: [&str; 3] = ["pending", "in_progress", "completed"];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "update_plan".into(),
        description: "Record or update a short, user-visible plan / TODO list for \
             the current multi-step task. Each call REPLACES the whole plan, so \
             always send every step. Use it to track progress on work that takes \
             several steps, and keep it short. The plan is shown to the user as a \
             checklist — it does not gate or block your reply; finish whenever the \
             answer is ready. Skip it entirely for simple, single-step tasks."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "Optional one-line note on what changed in the plan."
                },
                "plan": {
                    "type": "array",
                    "description": "The full plan — every step, in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": { "type": "string", "description": "What this step does." },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "This step's status."
                            }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["plan"],
            "additionalProperties": false
        }),
    }
}

/// UI metadata (REQUIREMENTS §4.1). `ResultArtifact::Plan` is what tells the
/// agent loop to emit `plan_updated` instead of a canvas tool card.
pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "更新计划",
        result: ResultArtifact::Plan,
        summary: |args| {
            let steps = args
                .get("plan")
                .and_then(|v| v.as_array())
                .map_or(0, |p| p.len());
            format!("更新计划 · {steps} 步")
        },
    }
}

pub fn run(args: &serde_json::Value) -> ToolOutcome {
    let Some(plan) = args.get("plan").and_then(|v| v.as_array()) else {
        return ToolOutcome::error("update_plan: missing required array argument 'plan'.");
    };

    let mut steps = Vec::with_capacity(plan.len());
    for (i, item) in plan.iter().enumerate() {
        let step = item.get("step").and_then(|v| v.as_str());
        let status = item.get("status").and_then(|v| v.as_str());
        let (Some(step), Some(status)) = (step, status) else {
            return ToolOutcome::error(format!(
                "update_plan: plan[{i}] needs a string 'step' and a string 'status'."
            ));
        };
        if !STATUSES.contains(&status) {
            return ToolOutcome::error(format!(
                "update_plan: plan[{i}] status '{status}' is not one of pending|in_progress|completed."
            ));
        }
        steps.push(serde_json::json!({ "step": step, "status": status }));
    }

    let explanation = args.get("explanation").and_then(|v| v.as_str());
    let model_output = format!("Plan updated — {} step(s).", steps.len());
    // `display_payload` is the plan the agent loop forwards into the
    // `plan_updated` event; `model_output` just confirms to the model.
    ToolOutcome::ok(
        model_output,
        serde_json::json!({ "plan": steps, "explanation": explanation }),
        serde_json::json!({ "raw_args": args }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(steps: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "plan": steps })
    }

    #[test]
    fn accepts_a_valid_plan() {
        let out = run(&plan(serde_json::json!([
            { "step": "查行情", "status": "completed" },
            { "step": "读财报", "status": "in_progress" },
            { "step": "综合判断", "status": "pending" },
        ])));
        assert!(!out.is_error);
        assert_eq!(out.display_payload["plan"].as_array().unwrap().len(), 3);
        assert!(out.model_output.contains("3 step"));
    }

    #[test]
    fn rejects_missing_plan() {
        assert!(run(&serde_json::json!({})).is_error);
    }

    #[test]
    fn rejects_an_unknown_status() {
        let out = run(&plan(serde_json::json!([
            { "step": "x", "status": "blocked" },
        ])));
        assert!(out.is_error);
        assert!(out.model_output.contains("blocked"));
    }

    #[test]
    fn rejects_a_step_missing_fields() {
        let out = run(&plan(serde_json::json!([{ "step": "x" }])));
        assert!(out.is_error);
    }
}
