//! Runtime kernel page cleanup.
//!
//! leek's "default mind" is the corpus `principles-runtime-kernel` wiki page.
//! It is no longer pre-distilled into a separate artifact: `build_system_prompt`
//! reads it live from the embedded corpus and cleans it here, so updating the
//! corpus repo (then rebuilding) is the only step needed to refresh it.
//!
//! Cleaning rules:
//! - strip YAML frontmatter (already gone from `lookup_doc` bodies; kept for
//!   safety and any direct-file caller)
//! - drop `## 来源 / ## Sources / ## 相关概念 / ## Related / ## 思想演变 /
//!   ## 典型案例 / ## 开放问题` sections — provenance and path lists the
//!   prompt doesn't need (`corpus_search` is the retrieval entry point)
//! - simplify wikilinks `[[wikis/.../slug]]` → `slug` (with hyphens → spaces);
//!   the corpus forbids the `|alias` form but we defensively strip it if present
//! - keep everything else verbatim

/// Corpus id (no `.md`) of the runtime kernel page injected into the system
/// prompt as leek's default reasoning mind. Read via `corpus_search::lookup_doc`.
pub const RUNTIME_KERNEL_ID: &str = "wikis/principles/concepts/principles-runtime-kernel";

/// Strip the leading YAML frontmatter block `---\n...\n---\n?` if present.
fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    if let Some(end) = rest.find("\n---\n") {
        return &rest[end + 5..];
    }
    if let Some(end) = rest.find("\n---") {
        // EOF without trailing newline
        let cut = end + 4;
        return rest.get(cut..).unwrap_or("");
    }
    text
}

/// Drop `heading` and everything until the next `## ` (or `# `) header / EOF.
fn strip_section(text: &str, heading: &str) -> String {
    let needle_with_nl = format!("\n{}", heading);
    let start = if text.starts_with(heading) {
        Some(0)
    } else {
        text.find(&needle_with_nl).map(|p| p + 1)
    };
    let Some(start) = start else {
        return text.to_string();
    };
    let after = start + heading.len();
    let rest = &text[after..];
    let next = rest.find("\n## ").or_else(|| rest.find("\n# "));
    match next {
        Some(end) => {
            let head = text[..start].trim_end();
            let tail = &text[after + end..];
            format!("{}\n\n{}", head, tail.trim_start())
        }
        None => text[..start].trim_end().to_string(),
    }
}

/// `[[wikis/principles/concepts/margin-of-safety]]` → `margin of safety`.
fn simplify_wikilinks(text: &str) -> String {
    let re = regex::Regex::new(r"\[\[([^\]]+)\]\]").unwrap();
    re.replace_all(text, |caps: &regex::Captures| {
        let inner = &caps[1];
        let no_alias = inner.split('|').next().unwrap_or(inner);
        let last = no_alias.rsplit('/').next().unwrap_or(no_alias);
        last.replace('-', " ")
    })
    .to_string()
}

pub fn clean_page(raw: &str) -> String {
    let mut text = strip_frontmatter(raw).trim_start().to_string();
    for heading in [
        // path-only scaffolding (no semantic content for the LLM)
        "## 来源",
        "## Sources",
        "## 相关概念",
        "## 相关概念地图",
        "## Related Concepts",
        "## Related",
        "## Corpus 覆盖范围",
        "## 思想演变",
        "## 典型案例",
        "## 开放问题",
        "## Open Questions",
    ] {
        text = strip_section(&text, heading);
    }
    simplify_wikilinks(&text).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_yaml_block() {
        let input = "---\ntitle: Foo\nslug: foo\n---\n\nContent here";
        assert_eq!(strip_frontmatter(input).trim_start(), "Content here");
    }

    #[test]
    fn strip_frontmatter_passthrough_when_absent() {
        assert_eq!(strip_frontmatter("Content here"), "Content here");
    }

    #[test]
    fn strip_section_removes_section_until_next_h2() {
        let input = "## 概念解析\nbody\n\n## 来源\n- [[foo]]\n- [[bar]]\n\n## 思想演变\nmore body";
        let out = strip_section(input, "## 来源");
        assert!(!out.contains("来源"));
        assert!(!out.contains("[[foo]]"));
        assert!(out.contains("概念解析"));
        assert!(out.contains("思想演变"));
    }

    #[test]
    fn strip_section_at_end_of_doc() {
        let input = "## 概念解析\nbody\n\n## 来源\n- [[foo]]";
        let out = strip_section(input, "## 来源");
        assert!(!out.contains("来源"));
        assert!(out.contains("概念解析"));
    }

    #[test]
    fn strip_section_no_match_returns_original() {
        let input = "## 概念解析\nbody";
        assert_eq!(strip_section(input, "## 来源"), input);
    }

    #[test]
    fn simplify_wikilinks_uses_last_segment_with_spaces() {
        assert_eq!(
            simplify_wikilinks("see [[wikis/principles/concepts/margin-of-safety]]"),
            "see margin of safety"
        );
        assert_eq!(
            simplify_wikilinks("plain [[economic-moat]]"),
            "plain economic moat"
        );
        // Defensive: alias form (corpus says no aliases, but be safe)
        assert_eq!(simplify_wikilinks("[[foo/bar|Bar]]"), "bar");
    }

    #[test]
    fn clean_page_full_pipeline() {
        let raw = "---\ntitle: Margin of Safety\nslug: margin-of-safety\ntier: principles\n---\n\n## 概念解析\n### 定义\nThe core idea.\n\n## 相关概念\n- [[wikis/principles/concepts/circle-of-competence]]\n\n## 来源\n- [[sources/principles/buffett/letters/2014]]\n";
        let cleaned = clean_page(raw);
        assert!(!cleaned.contains("title:"));
        assert!(!cleaned.contains("来源"));
        assert!(!cleaned.contains("相关概念"));
        assert!(!cleaned.contains("[["));
        assert!(cleaned.contains("The core idea"));
        assert!(cleaned.contains("概念解析"));
    }
}
