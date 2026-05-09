//! Critic / evaluator pass.
//!
//! After the main agent emits a `MessageEnd` for a task whose
//! `expected_deliverable` warrants rigor (`decision_draft`, `research_brief`),
//! we run a cheap second LLM call that audits the draft against the discipline
//! checklist: fact/inference/speculation distinction, opposing case,
//! invalidation conditions, mandate alignment, citations. If the critic finds
//! gaps, the agent loop pushes the critique into `additional_inputs` and
//! re-runs the main agent (capped at `MAX_CRITIC_REWRITES`). This is the
//! structural lever for "shallow analysis" — the model is no longer trusted
//! to self-police.

use std::sync::Arc;

use anyhow::{Context, Result};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, ReasoningEffort, Role, Verbosity};

/// Cap. One critic-driven rewrite is the right call for this loop: the model
/// either materially fixes the gaps on the second pass (good) or it doubles
/// down on the same shape (in which case running a third pass burns tokens
/// without gaining signal).
pub const MAX_CRITIC_REWRITES: usize = 1;

const CRITIC_MODEL: &str = "gpt-5.5";

const CRITIC_SYSTEM_PROMPT: &str = "\
You are the **critic** subagent inside leek. The main agent has produced a \
draft reply for a task. Your only job is to score the draft against the \
discipline checklist below and decide whether it ships or needs another pass.

CHECKLIST (apply each one strictly):
1. **Fact / inference / speculation distinction** — does the draft make clear \
which claims are verified facts (with sources), which are inference from \
those facts, and which are speculation? Hand-waving counts as a gap.
2. **Opposing case** — for any directional call (long/short/avoid/ignore/etc), \
is there a steelmanned bear-or-bull case engaging with the *reasons* a smart \
investor would take the other side? \"Risks include market volatility\" is a \
cop-out, not an opposing case.
3. **Invalidation conditions** — what observable would falsify the thesis? \
Must be specific (price level, EPS revision direction, regulatory event). \
\"If conditions change\" is a gap.
4. **Mandate alignment** — does the draft check the user's mandate (risk \
tolerance, position caps, instrument restrictions)? Skip only if the user \
has no mandate file *and* the answer doesn't ask the user to commit capital.
5. **Citations** — every quantitative claim or non-obvious fact has a source \
(corpus title, web link, tool name). Inferences may go uncited but must be \
labeled inference.

OUTPUT FORMAT — exactly one JSON object on a single line, nothing else, no \
code fences. Schema:
{\"verdict\": \"ship\" | \"rewrite\", \"gaps\": [\"<one-line gap>\", ...]}

If verdict = \"ship\": gaps SHOULD be empty (you can list nits if any).
If verdict = \"rewrite\": gaps MUST list the specific failures, not boilerplate.

Be strict. The default verdict is \"rewrite\" unless every checklist item is \
clearly satisfied. Length / pretty prose / surface confidence are not \
substitutes for the checklist.";

#[derive(Debug, Clone)]
pub struct Critique {
    pub verdict: String,
    pub gaps: Vec<String>,
}

impl Critique {
    pub fn needs_rewrite(&self) -> bool {
        self.verdict == "rewrite" && !self.gaps.is_empty()
    }
}

/// Decide whether to run the critic for this task. Decision drafts and
/// research briefs are the high-leverage cases. Reviews and comparisons are
/// also worth auditing. Free-form chat replies and morning briefs are not —
/// the critic would burn tokens without raising the bar.
pub fn should_run_critic(expected_deliverable: Option<&str>) -> bool {
    matches!(
        expected_deliverable,
        Some("decision_draft" | "research_brief" | "review" | "comparison")
    )
}

pub async fn evaluate_draft(
    provider: Arc<dyn LlmProvider>,
    expected_deliverable: &str,
    user_question: &str,
    draft: &str,
    cancel: CancellationToken,
) -> Result<Option<Critique>> {
    if draft.trim().is_empty() {
        return Ok(None);
    }
    let user_msg = format!(
        "# Task type\n{expected_deliverable}\n\n# User's request\n{user_question}\n\n\
         # Draft reply (from main agent)\n{draft}\n\n\
         Score the draft. Output the JSON verdict object only."
    );
    let req = ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: user_msg,
        }],
        system: Some(CRITIC_SYSTEM_PROMPT.to_string()),
        model: CRITIC_MODEL.to_string(),
        max_output_tokens: None,
        tools: Vec::new(),
        additional_inputs: Vec::new(),
        // Critic is cheap — Low reasoning is plenty to score the JSON verdict
        // and avoids gpt-5.5's rejection of `minimal`.
        reasoning_effort: Some(ReasoningEffort::Low),
        verbosity: Some(Verbosity::Low),
    };

    let mut stream = provider.chat(req).await.context("critic: chat call")?;
    let mut text = String::new();
    while let Some(evt) = stream.next().await {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        match evt? {
            LlmEvent::TextDelta { text: t } => text.push_str(&t),
            LlmEvent::MessageEnd { .. } => break,
            _ => {}
        }
    }

    parse_verdict(&text).map(Some)
}

fn parse_verdict(raw: &str) -> Result<Critique> {
    let trimmed = raw.trim();
    let candidate = extract_json_object(trimmed).unwrap_or(trimmed);
    let v: serde_json::Value = serde_json::from_str(candidate)
        .with_context(|| format!("critic returned non-JSON: {}", trimmed.chars().take(200).collect::<String>()))?;
    let verdict = v
        .get("verdict")
        .and_then(|s| s.as_str())
        .unwrap_or("rewrite")
        .to_string();
    let gaps = v
        .get("gaps")
        .and_then(|g| g.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(Critique { verdict, gaps })
}

/// Same balanced-brace extractor as routing — rescues a JSON object from
/// occasional leading/trailing prose.
fn extract_json_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if in_str {
            match b {
                b'\\' => escape = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn critic_input(critique: &Critique, draft: &str) -> serde_json::Value {
    let gaps_md = critique
        .gaps
        .iter()
        .map(|g| format!("- {g}"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = format!(
        "[critic] The previous draft did not pass the discipline checklist. \
         Address the following gaps and produce a new reply. Do NOT defend \
         the prior draft — fix it.\n\n\
         # Gaps\n{gaps_md}\n\n\
         # Prior draft (for reference, not to be regurgitated)\n{draft}"
    );
    serde_json::json!({
        "type": "message",
        "role": "user",
        "content": body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ship_verdict() {
        let raw = r#"{"verdict":"ship","gaps":[]}"#;
        let c = parse_verdict(raw).unwrap();
        assert_eq!(c.verdict, "ship");
        assert!(c.gaps.is_empty());
        assert!(!c.needs_rewrite());
    }

    #[test]
    fn parses_rewrite_with_gaps() {
        let raw = r#"{"verdict":"rewrite","gaps":["missing opposing case","no invalidation"]}"#;
        let c = parse_verdict(raw).unwrap();
        assert!(c.needs_rewrite());
        assert_eq!(c.gaps.len(), 2);
    }

    #[test]
    fn parses_with_leading_prose() {
        let raw = "Here is the audit:\n{\"verdict\":\"rewrite\",\"gaps\":[\"x\"]}\n";
        let c = parse_verdict(raw).unwrap();
        assert!(c.needs_rewrite());
    }

    #[test]
    fn should_run_for_decision_and_brief() {
        assert!(should_run_critic(Some("decision_draft")));
        assert!(should_run_critic(Some("research_brief")));
        assert!(should_run_critic(Some("review")));
        assert!(should_run_critic(Some("comparison")));
        assert!(!should_run_critic(Some("free_form")));
        assert!(!should_run_critic(Some("morning_brief")));
        assert!(!should_run_critic(None));
    }
}
