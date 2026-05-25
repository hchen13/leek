//! System prompt builder — assembles leek's "investment-research mind"
//! baseline from `harness/` markdown assets and distilled corpus principles.
//!
//! The pieces:
//! - identity.md — who leek is + mission
//! - discipline.md — operational discipline (fact/inference/speculation,
//!   citation, uncertainty, user-constraint boundary, autonomous continuation,
//!   tool-use discipline)
//! - corpus_orientation.md — corpus dual-axis layout + thinking order
//! - distilled corpus principles
//!
//! Per-tool "when to use / when not to" lives in each tool's `description`
//! field (tools/*.rs) — not in this builder. The 60-line ad-hoc SYSTEM_PROMPT
//! this replaces was largely a tool-decision tree; that responsibility now
//! belongs to the tool spec.

const IDENTITY: &str = include_str!("../../../../harness/identity.md");
const DISCIPLINE: &str = include_str!("../../../../harness/discipline.md");
const CORPUS_ORIENTATION: &str = include_str!("../../../../harness/corpus_orientation.md");

use std::path::PathBuf;

const DEFAULT_CORPUS_PROMPT_PATH: &str = "crates/gateway/assets/corpus_distilled.md";
const ENV_CORPUS_PROMPT_PATH: &str = "LEEK_CORPUS_PROMPT_PATH";
const MAX_CORPUS_PROMPT_CHARS: usize = 14_000;

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
- The default research chain is: corpus principles → current facts → opposing \
case → risk/exit conditions → answer. Add user constraints only when the \
current session explicitly provides them.\n\
- If the task scope, desired depth, risk tolerance, time horizon, or required \
private context is genuinely unclear and materially changes the answer, call \
`ask_user_question` before committing to a path. Do not ask when a reasonable \
default or read-only research can resolve the ambiguity.\n\
- Use `update_plan` only when the work is long-running or complex enough that \
a visible checklist will improve execution. Do not create a plan for ordinary \
questions, short explanations, or quick lookups. When you create a plan, keep \
it honest: update it when the real state changes, revise it when the direction \
changes, and abandon it explicitly when it no longer fits.\n\
- Ground before asking: inspect corpus, available tools, session history, and \
current market facts before asking the user. Ask only for preferences or \
private context that materially change the work.\n\
- Persist through failures. A failed function call is not a stopping condition: \
read the error, try a better query, another source, another tool, or state the \
blocked evidence boundary before concluding. If an active plan exists, update \
it to match that boundary.\n\
- Explore credible alternatives. If the first lens/source/tool produces a weak \
or one-sided thesis, try another relevant lens or source path before answering.\n\
- Use a hard evidence budget per user turn. For normal public-company research, \
do at most three web-search batches and open/fetch at most three external pages \
before synthesizing. Exceed this only when the user explicitly asks for deep \
source collection or the current evidence is contradictory. Do not repeat \
semantically identical searches, URLs, PDFs, quote pulls, K-line pulls, or \
financial calls unless freshness or a different field is required.\n\
- Stop gathering once the answer has enough evidence for the user's requested \
decision stage. A framework-only turn should not sneak in a final action. A \
screening turn should not become a full report. If the remaining gaps are \
non-blocking, name them in the answer instead of extending the tool loop.\n\
- A final answer is allowed only after the declared research frame has been \
acted on. If you say a working model needs industry, channel, macro, policy, \
competition, or liquidity facts, gather those facts with tools before you \
answer. Do not hand the checklist back to the user as the answer.\n\
- For public-company research, corpus gaps normally require live external \
grounding: use web_search/web_fetch for industry state, policy/macro context, \
channel or competitive facts unless the user forbids web access or the tool \
fails after a real attempt.\n\
- For A-share research, prefer company announcements, exchange/CNINFO \
disclosures, company IR pages, official government/statistical sources, and \
structured financial tools. Treat industry reports and media as secondary \
judgment sources, and filter weakly related sources such as Reddit, arXiv, \
Wikipedia, forums, SEO aggregators, or unreadable fetch outputs.\n\
- For listed-bank comparisons, generic income/balance/cashflow ratios are not \
enough for asset quality. Actively seek non-performing-loan ratio, provision \
coverage, special-mention/overdue loans, net interest margin, and capital \
adequacy from announcements, annual/interim reports, exchange/CNINFO/IR, \
or bank-sector research; if unavailable, state the blocked evidence boundary \
instead of pretending ROE/PB is an asset-quality proxy.\n\
- Use `delegate_research` when a task benefits from a genuinely separate \
worker. Define that worker's role, task, thinking frame, and output shape in \
the call itself. The main agent remains accountable for the final answer.\n\n\
# Visible progress narration\n\n\
- For non-trivial research work, before each research tool call or coherent \
tool batch, write one short Chinese progress note explaining what you are \
checking and why it matters.\n\
- The progress note is a user-visible reasoning trace, not private chain of \
thought. State the next check, the decision relevance, and any key uncertainty; \
do not expose hidden deliberation.\n\
- Keep each note concise. Do not repeat the final answer in progress notes.";

/// Build the stable system prompt. Runtime state such as compaction handoff,
/// plans, and tool outputs belongs in input messages so
/// the provider can cache this static instruction prefix aggressively.
pub fn build_system_prompt() -> String {
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
    if let Some(corpus_prompt) = load_corpus_prompt() {
        prompt.push_str("\n\n");
        prompt.push_str(&corpus_prompt);
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
        Some(limit_corpus_prompt(trimmed))
    }
}

fn limit_corpus_prompt(trimmed: &str) -> String {
    if trimmed.chars().count() <= MAX_CORPUS_PROMPT_CHARS {
        return trimmed.to_string();
    }
    let mut out = trimmed
        .chars()
        .take(MAX_CORPUS_PROMPT_CHARS)
        .collect::<String>();
    out.push_str(
        "\n\n[Corpus kernel truncated for runtime budget. Use corpus_search and corpus_read for any specific principle, source wording, or deeper framework needed by the task.]",
    );
    out
}

pub fn build_subagent_prompt(_role: &str, role_instruction: &str) -> String {
    format!(
        "{IDENTITY}\n\n{DISCIPLINE}\n\n{CORPUS_ORIENTATION}\n\n\
         # Subagent role\n\n\
         You are a focused research subagent inside L.E.E.K. \
         {role_instruction}\n\n\
         Output in Chinese. Follow the task shape requested by the main agent. \
         Do not claim you fetched data unless the main agent gave it to you in context."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_includes_all_baseline_sections() {
        let p = build_system_prompt();
        assert!(p.contains("# Identity"));
        assert!(p.contains("truth-seeking investment partner"));
        assert!(p.contains("# Discipline"));
        assert!(p.contains("## 7. 工具使用纪律"));
        assert!(p.contains("# corpus orientation"));
        assert!(p.contains("Citation surface conventions"));
        assert!(p.contains("General research method"));
        assert!(p.contains("delegate_research"));
        assert!(p.contains("A 股 source discipline"));
        assert!(p.contains("exchange/CNINFO"));
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
    fn corpus_prompt_is_budget_limited() {
        let text = "a".repeat(MAX_CORPUS_PROMPT_CHARS + 10);
        let limited = limit_corpus_prompt(&text);
        assert!(limited.contains("Corpus kernel truncated"));
        assert!(limited.chars().count() < text.chars().count() + 200);
    }

    #[test]
    fn build_does_not_inject_compaction_handoff() {
        let p = build_system_prompt();
        assert!(!p.contains("Prior session handoff"));
        assert!(!p.contains("旧 session 摘要"));
    }

    #[test]
    fn build_omits_handoff_when_empty() {
        let p = build_system_prompt();
        assert!(!p.contains("Prior session handoff"));
    }

    #[test]
    fn build_does_not_inject_legacy_user_profile_section() {
        let p = build_system_prompt();
        assert!(!p.contains("# User profile"));
        assert!(!p.contains("vault/<user_id>/profile.md"));
    }

    #[test]
    fn build_does_not_advertise_removed_research_note_tool() {
        let p = build_system_prompt();
        assert!(!p.contains("record_research_note"));
    }

    #[test]
    fn build_keeps_runtime_context_out_of_static_prompt() {
        let p = build_system_prompt();
        assert!(!p.contains("Investment philosophy"));
        assert!(!p.contains("COMPACTED PRIOR SESSION HISTORY"));
    }

    /// Disabled by default; run with
    /// `cargo test -p leek-gateway agent::harness::tests::dump -- --nocapture --ignored`
    /// to eyeball the assembled prompt.
    #[test]
    #[ignore]
    fn dump_prompt() {
        let p = build_system_prompt();
        eprintln!("--- PROMPT (len={} bytes) ---\n{}\n--- END ---", p.len(), p);
    }
}
