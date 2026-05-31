//! System prompt builder — assembles leek's "investment-research mind"
//! baseline from `harness/` markdown assets and distilled corpus principles.
//!
//! The pieces:
//! - identity.md — who leek is + mission
//! - corpus_orientation.md — how to use corpus as the reasoning substrate
//! - discipline.md — evidence, tool, output, and citation posture
//! - the runtime kernel page, read live from the embedded corpus
//!
//! Per-tool "when to use / when not to" lives in each tool's `description`
//! field (tools/*.rs) — not in this builder. The 60-line ad-hoc SYSTEM_PROMPT
//! this replaces was largely a tool-decision tree; that responsibility now
//! belongs to the tool spec.

const IDENTITY: &str = include_str!("../../../../harness/identity.md");
const DISCIPLINE: &str = include_str!("../../../../harness/discipline.md");
const CORPUS_ORIENTATION: &str = include_str!("../../../../harness/corpus_orientation.md");

use crate::agent::tools::corpus_search;
use crate::corpus::kernel;

const MAX_CORPUS_PROMPT_CHARS: usize = 14_000;

/// Frames the runtime kernel page for the model. The body itself is the corpus
/// `principles-runtime-kernel` page, read live and cleaned at build time — there
/// is no separate distilled artifact to keep in sync.
const KERNEL_PREAMBLE: &str = "# Principles runtime kernel (your default mind)\n\n\
This is the compact operating kernel you start every investment conversation \
with: the 7-step reasoning ladder, Core propositions, and productive tensions \
shared by Buffett, Munger, Dalio, and Duan Yongping. Use it as an orientation \
layer; use `corpus_search` and `corpus_read` when the task needs a specific \
concept page, source quote, knowledge page, company fact, or current-world \
evidence.\n\n---\n\n";

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
    if let Some(corpus_prompt) = load_corpus_prompt() {
        prompt.push_str("\n\n");
        prompt.push_str(&corpus_prompt);
    }

    prompt
}

/// Read the runtime kernel page live from the embedded corpus, clean it, and
/// frame it with the preamble. Returns `None` only if the corpus checkout is
/// missing the kernel page (e.g. an empty / placeholder corpus).
fn load_corpus_prompt() -> Option<String> {
    let doc = corpus_search::lookup_doc(kernel::RUNTIME_KERNEL_ID)?;
    let cleaned = kernel::clean_page(&doc.body);
    if cleaned.is_empty() {
        return None;
    }
    let mut blob = String::with_capacity(KERNEL_PREAMBLE.len() + cleaned.len());
    blob.push_str(KERNEL_PREAMBLE);
    blob.push_str(&cleaned);
    Some(limit_corpus_prompt(&blob))
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
        assert!(p.contains("corpus-grounded investment research partner"));
        assert!(p.contains("# Discipline"));
        assert!(p.contains("# Corpus Orientation"));
        assert!(p.contains("stable operating logic"));
        assert!(p.contains("Principles Runtime Kernel"));
        assert!(p.contains("Buffett, Munger, Dalio, and Duan Yongping"));
        assert!(p.contains("Output And Citation"));
        assert!(p.contains("delegate_research"));
    }

    #[test]
    fn corpus_prompt_loads_runtime_kernel_from_embedded_corpus() {
        let p =
            load_corpus_prompt().expect("runtime kernel page should load from the embedded corpus");
        // Preamble framing is present.
        assert!(p.contains("Principles runtime kernel (your default mind)"));
        // Kernel body made it through (中文 ladder content).
        assert!(p.contains("看清对象") || p.contains("7 步推理阶梯"));
        // clean_page stripped the provenance / path-list sections.
        assert!(!p.contains("## 来源"));
        assert!(!p.contains("## 相关概念"));
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
