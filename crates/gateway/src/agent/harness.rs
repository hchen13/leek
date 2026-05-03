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

/// Build the full system prompt. `handoff_summaries` are pre-extracted
/// compaction-summary message texts (role=compaction_summary rows), to be
/// joined into the inherited-context section.
pub fn build_system_prompt(handoff_summaries: &[String]) -> String {
    let mut prompt = String::with_capacity(8192);

    prompt.push_str(IDENTITY);
    prompt.push_str("\n\n");
    prompt.push_str(DISCIPLINE);
    prompt.push_str("\n\n");
    prompt.push_str(CORPUS_ORIENTATION);
    prompt.push_str("\n\n");
    prompt.push_str(CITATION_CONVENTIONS);

    // TODO: when the corpus distiller lands, include the distilled
    // principles blob here as `# Investing principles (your default mind)`.
    // TODO: when per-user vault wiring lands, read
    // `vault/{user_id}/mandate.md` and append as `# User mandate`.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_includes_all_baseline_sections() {
        let p = build_system_prompt(&[]);
        assert!(p.contains("# Identity"));
        assert!(p.contains("truth-seeking investment partner"));
        assert!(p.contains("# Discipline"));
        assert!(p.contains("## 7. 工具使用纪律"));
        assert!(p.contains("# corpus orientation"));
        assert!(p.contains("Citation surface conventions"));
    }

    #[test]
    fn build_appends_handoff_summaries() {
        let summaries = vec!["旧 session 摘要".to_string()];
        let p = build_system_prompt(&summaries);
        assert!(p.contains("Prior session handoff"));
        assert!(p.contains("旧 session 摘要"));
    }

    #[test]
    fn build_omits_handoff_when_empty() {
        let p = build_system_prompt(&[]);
        assert!(!p.contains("Prior session handoff"));
    }

    /// Disabled by default; run with
    /// `cargo test -p leek-gateway agent::harness::tests::dump -- --nocapture --ignored`
    /// to eyeball the assembled prompt.
    #[test]
    #[ignore]
    fn dump_prompt() {
        let p = build_system_prompt(&[]);
        eprintln!("--- PROMPT (len={} bytes) ---\n{}\n--- END ---", p.len(), p);
    }
}
