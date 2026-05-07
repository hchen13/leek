//! System prompt builder — assembles leek's "investment-research mind"
//! baseline from `harness/` markdown assets and runtime context (compaction
//! handoff, eventually mandate + distilled corpus principles).
//!
//! The pieces:
//! - identity.md — who leek is + mission
//! - discipline.md — operational discipline (fact/inference/speculation,
//!   citation, uncertainty, mandate boundary, autonomous continuation,
//!   tool-use discipline)
//! - corpus_orientation.md — corpus dual-axis layout + thinking order
//! - distilled corpus principles — TODO once the distiller exists
//! - user mandate — TODO once `vault/{user_id}/mandate.md` is wired
//! - handoff summaries from prior compactions — pre-extracted by the caller
//!
//! Per-tool "when to use / when not to" lives in each tool's `description`
//! field (tools/*.rs) — not in this builder. The 60-line ad-hoc SYSTEM_PROMPT
//! this replaces was largely a tool-decision tree; that responsibility now
//! belongs to the tool spec.

const IDENTITY: &str = include_str!("../../../../harness/identity.md");
const DISCIPLINE: &str = include_str!("../../../../harness/discipline.md");
const CORPUS_ORIENTATION: &str = include_str!("../../../../harness/corpus_orientation.md");

use crate::agent::tools::use_skill::skill_metadata;
use std::path::PathBuf;

const DEFAULT_CORPUS_PROMPT_PATH: &str = "crates/gateway/assets/corpus_distilled.md";
const ENV_CORPUS_PROMPT_PATH: &str = "LEEK_CORPUS_PROMPT_PATH";

/// Citation *surface* conventions — runtime rules for how to render citations
/// (don't leak internal path ids, prefer titled markdown links). Different
/// from discipline §2 which governs *when* to attribute; these are
/// implementation details, kept in code rather than a markdown file the user
/// reviews as part of the harness.
const CITATION_CONVENTIONS: &str = "\
# Citation surface conventions\n\n\
- When citing a corpus document, use its human TITLE (e.g. \"Margin of \
Safety\", \"Long-term debt cycle\"), NEVER its internal path id like \
`wikis/principles/concepts/...`. Path ids are an implementation detail the \
user should not see.\n\
- When citing a web URL, use a [Title](URL) markdown link with a short \
human-readable title; never paste a raw URL as the visible text.";

const METHOD_AND_DELEGATION: &str = "\
# General research method\n\n\
- Do not treat financial research as a bag of scenarios. For any unfamiliar \
task, first identify the decision type, then choose the right corpus lens, \
then gather situation data, then stress-test the thesis, then translate it \
into a decision frame only if the user is asking for one.\n\
- The default chain is: user mandate → corpus principles → current facts → \
opposing case → risk/exit conditions → answer.\n\
- If the task scope, desired depth, risk tolerance, time horizon, or required \
private context is genuinely unclear and materially changes the answer, call \
`ask_user_question` before creating or executing the plan. Do not ask when a \
reasonable default or read-only research can resolve the ambiguity.\n\
- For non-trivial research tasks, create and maintain an active plan with \
`update_plan` using the `plan` argument. Keep exactly one item in_progress unless all items are \
completed. A final task answer is allowed only after the active plan is \
completed.\n\
- Ground before asking: inspect corpus, available tools, session history, and \
current market facts before asking the user. Ask only for preferences or \
private context that materially change the work.\n\
- Persist through failures. A failed function call is not a stopping condition: \
read the error, try a better query, another source, another tool, or mark the \
specific plan item as blocked with evidence before concluding.\n\
- Explore credible alternatives. If the first lens/source/tool produces a weak \
or one-sided thesis, try another relevant lens or source path before answering.\n\
- A final answer is allowed only after the declared research frame has been \
acted on. If you say a working model needs industry, channel, macro, policy, \
competition, or liquidity facts, gather those facts with tools before you \
answer. Do not hand the checklist back to the user as the deliverable.\n\
- For public-company research, corpus gaps normally require live external \
grounding: use web_search/web_fetch for industry state, policy/macro context, \
channel or competitive facts unless the user forbids web access or the tool \
fails after a real attempt.\n\
- Use `record_research_note` for reversible memory, preferences, mandate \
candidates, or reusable lessons. Use `record_investment_action` only for a \
clear capital commitment to a named instrument and direction.\n\
	- Use `delegate_research` when a task needs a specialized second lens. The \
	main agent remains accountable for the final answer; subagents provide \
	focused reports, not final authority.\n\
	- Good delegation examples: data gaps to `data_scout`, valuation quality to \
	`fundamental_analyst`, short-term opportunity structure to `trading_analyst`, \
	bear case and sizing risk to `risk_manager`, and corpus drift checks to \
	`corpus_guardian`.\n\n\
	# Visible progress narration\n\n\
	- For non-trivial research work, before each research tool call or coherent \
	tool batch after the initial `use_skill`, write one short Chinese progress \
	note explaining what you are checking and why it matters.\n\
	- The progress note is a user-visible reasoning trace, not private chain of \
	thought. State the next check, the decision relevance, and any key uncertainty; \
	do not expose hidden deliberation.\n\
	- Keep each note concise. Do not repeat the final answer in progress notes.";

/// Build the full system prompt. `handoff_summaries` are pre-extracted
/// compaction-summary message texts (role=compaction_summary rows), to be
/// joined into the inherited-context section. `mandate` is the contents of
/// `vault/<user_id>/mandate.md` if it exists and is non-empty — caller is
/// responsible for reading the file (we keep this fn pure for testability).
pub fn build_system_prompt(
    handoff_summaries: &[String],
    mandate: Option<&str>,
    charter: Option<&str>,
) -> String {
    let mut prompt = String::with_capacity(8192);

    prompt.push_str(IDENTITY);
    prompt.push_str("\n\n");
    prompt.push_str(DISCIPLINE);
    prompt.push_str("\n\n");
    prompt.push_str(CORPUS_ORIENTATION);
    prompt.push_str("\n\n");
    prompt.push_str(CITATION_CONVENTIONS);
    prompt.push_str("\n\n");
    prompt.push_str(METHOD_AND_DELEGATION);
    prompt.push_str("\n\n");
    prompt.push_str("# Research Skills\n\n");
    prompt.push_str(
        "Available skills (call `use_skill` with the skill name as your FIRST action \
         when the task matches — before any other tool call or analysis):\n\n",
    );
    prompt.push_str(&skill_metadata());

    if let Some(corpus_prompt) = load_corpus_prompt() {
        prompt.push_str("\n\n");
        prompt.push_str(&corpus_prompt);
    }

    // User mandate — discipline §5 promotes this to the source of truth
    // for risk tolerance / position caps / mandate hints. Empty / missing
    // mandate omits the section so the LLM doesn't hallucinate constraints.
    if let Some(text) = mandate {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            prompt.push_str("\n\n# User mandate (vault/<user_id>/mandate.md)\n\n");
            prompt.push_str(trimmed);
        }
    }

    if let Some(text) = charter {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            prompt.push_str("\n\n# Investment philosophy (team charter)\n\n");
            prompt.push_str(trimmed);
        }
    }

    if !handoff_summaries.is_empty() {
        prompt.push_str("\n\n# Prior session handoff (compacted)\n\n");
        prompt.push_str(&handoff_summaries.join("\n\n---\n\n"));
        prompt.push_str(
            "\n\nThe above summarizes the conversation up to this point. \
             Continue from where it leaves off; treat established facts and \
             the user's ongoing thread as known.",
        );
    }

    prompt
}

fn load_corpus_prompt() -> Option<String> {
    corpus_prompt_candidates()
        .into_iter()
        .find_map(|path| read_corpus_prompt(&path))
}

fn corpus_prompt_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(path) = std::env::var(ENV_CORPUS_PROMPT_PATH) {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            out.push(PathBuf::from(trimmed));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(DEFAULT_CORPUS_PROMPT_PATH));
    }
    out.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/corpus_distilled.md"));
    out
}

fn read_corpus_prompt(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with("<!-- placeholder") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn build_subagent_prompt(role: &str, role_instruction: &str) -> String {
    format!(
        "{IDENTITY}\n\n{DISCIPLINE}\n\n{CORPUS_ORIENTATION}\n\n\
         # Subagent role\n\n\
         You are `{role}`, a focused research subagent inside L.E.E.K. \
         {role_instruction}\n\n\
         Output in Chinese. Keep the report compact but decision-useful. \
         Separate facts, inference, speculation, missing data, and the strongest \
         opposing view. Do not claim you fetched data unless the main agent gave \
         it to you in context."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_includes_all_baseline_sections() {
        let p = build_system_prompt(&[], None, None);
        assert!(p.contains("# Identity"));
        assert!(p.contains("truth-seeking investment partner"));
        assert!(p.contains("# Discipline"));
        assert!(p.contains("## 7. 工具使用纪律"));
        assert!(p.contains("# corpus orientation"));
        assert!(p.contains("Citation surface conventions"));
        assert!(p.contains("General research method"));
        assert!(p.contains("delegate_research"));
        assert!(p.contains("# Research Skills"));
        assert!(p.contains("equity-valuation"));
        assert!(p.contains("crypto-research"));
    }

    #[test]
    fn read_corpus_prompt_ignores_placeholder() {
        let tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join(format!("leek-placeholder-{}.md", std::process::id()));
        std::fs::write(&path, "<!-- placeholder corpus_distilled.md -->\n").unwrap();
        assert!(read_corpus_prompt(&path).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn read_corpus_prompt_returns_trimmed_text() {
        let tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join(format!("leek-corpus-prompt-{}.md", std::process::id()));
        std::fs::write(&path, "\n# Corpus mind\n\nprinciples\n\n").unwrap();
        assert_eq!(
            read_corpus_prompt(&path).as_deref(),
            Some("# Corpus mind\n\nprinciples")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn build_appends_handoff_summaries() {
        let summaries = vec!["旧 session 摘要".to_string()];
        let p = build_system_prompt(&summaries, None, None);
        assert!(p.contains("Prior session handoff"));
        assert!(p.contains("旧 session 摘要"));
    }

    #[test]
    fn build_omits_handoff_when_empty() {
        let p = build_system_prompt(&[], None, None);
        assert!(!p.contains("Prior session handoff"));
    }

    #[test]
    fn build_includes_mandate_when_provided() {
        let m = "- 单标位置上限 5%\n- 不碰复杂衍生品";
        let p = build_system_prompt(&[], Some(m), None);
        assert!(p.contains("# User mandate"));
        assert!(p.contains("单标位置上限 5%"));
        assert!(p.contains("不碰复杂衍生品"));
    }

    #[test]
    fn build_omits_mandate_when_empty_or_whitespace() {
        let p1 = build_system_prompt(&[], Some(""), None);
        let p2 = build_system_prompt(&[], Some("   \n\n   "), None);
        let p3 = build_system_prompt(&[], None, None);
        assert!(!p1.contains("# User mandate"));
        assert!(!p2.contains("# User mandate"));
        assert!(!p3.contains("# User mandate"));
    }

    #[test]
    fn build_includes_charter_when_provided() {
        let charter = "We believe in long-term value investing.\n\n关注有护城河的企业。";
        let p = build_system_prompt(&[], None, Some(charter));
        assert!(p.contains("Investment philosophy"));
        assert!(p.contains("护城河"));
    }

    #[test]
    fn build_omits_charter_when_none() {
        let p = build_system_prompt(&[], None, None);
        assert!(!p.contains("Investment philosophy"));
    }

    /// Disabled by default; run with
    /// `cargo test -p leek-gateway agent::harness::tests::dump -- --nocapture --ignored`
    /// to eyeball the assembled prompt.
    #[test]
    #[ignore]
    fn dump_prompt() {
        let p = build_system_prompt(&[], None, None);
        eprintln!("--- PROMPT (len={} bytes) ---\n{}\n--- END ---", p.len(), p);
    }
}
