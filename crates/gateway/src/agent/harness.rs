//! System prompt builder — assembles leek's "investment-research mind"
//! baseline from `harness/` markdown assets and distilled corpus principles.
//!
//! The pieces:
//! - identity.md — who leek is + mission
//! - corpus_orientation.md — how to use corpus as the reasoning substrate
//! - discipline.md — evidence, tool, output, and citation posture
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
        assert!(p.contains("corpus-grounded investment research partner"));
        assert!(p.contains("# Discipline"));
        assert!(p.contains("# Corpus Orientation"));
        assert!(p.contains("Principles Runtime Kernel"));
        assert!(p.contains("Buffett, Munger, Dalio, and Duan Yongping"));
        assert!(p.contains("Output And Citation"));
        assert!(p.contains("delegate_research"));
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
