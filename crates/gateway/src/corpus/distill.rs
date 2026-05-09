//! Distill the runtime principles kernel into a single markdown blob suitable
//! for system-prompt injection.
//!
//! Cleaning rules:
//! - strip YAML frontmatter
//! - drop `## 来源 / ## Sources / ## 相关概念 / ## Related Concepts /
//!   ## Related` sections (path lists the LLM doesn't need)
//! - simplify wikilinks `[[wikis/.../slug]]` → `slug` (with hyphens →
//!   spaces); the corpus convention forbids `|alias` form so we don't honor
//!   it but defensively strip it if present
//! - keep everything else verbatim (definitions, tenets, applications,
//!   misconceptions, evolution of thought, canonical cases, open questions)
//!
//! Output gets a per-page H2 wrapper (`## <slug as title>`) and pages are
//! separated by horizontal rules. The whole blob is wrapped in a top-level
//! H1 with a metadata comment carrying the input hash for drift detection.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
const RUNTIME_KERNEL_PAGES: &[&str] = &[
    "wikis/principles/concepts/principles-runtime-kernel.md",
    "wikis/principles/entities/warren-buffett.md",
    "wikis/principles/entities/charlie-munger.md",
    "wikis/principles/entities/ray-dalio.md",
];

/// Strip the leading YAML frontmatter block `---\n...\n---\n?` if present.
fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    if let Some(end) = rest.find("\n---\n") {
        return &rest[end + 5..];
    }
    // EOF-without-trailing-newline fallback: only accept when "\n---" is
    // *exactly* at end of input. `find` matches the first occurrence, which
    // would otherwise mistake an in-body horizontal rule for the closer.
    if rest.ends_with("\n---") {
        return "";
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

fn clean_page(raw: &str) -> String {
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
        "## 开放问题",
        "## Open Questions",
    ] {
        text = strip_section(&text, heading);
    }
    simplify_wikilinks(&text).trim().to_string()
}

/// Stable content hash over the selected runtime-kernel pages. Used to detect
/// drift between distilled output and the corpus it was distilled from.
fn input_hash(corpus_root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for rel in RUNTIME_KERNEL_PAGES {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        let path = corpus_root.join(rel);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        hasher.update(&bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Debug)]
pub struct DistillReport {
    pub pages_in: usize,
    pub bytes_out: usize,
    pub input_hash: String,
}

/// Distill the corpus runtime kernel into a single embeddable markdown blob.
pub fn distill(corpus_root: &Path) -> Result<(String, DistillReport)> {
    for rel in RUNTIME_KERNEL_PAGES {
        let path = corpus_root.join(rel);
        if !path.exists() {
            anyhow::bail!("expected runtime kernel page at {}", path.display());
        }
    }

    let hash = input_hash(corpus_root)?;

    let mut pages = Vec::new();
    for rel in RUNTIME_KERNEL_PAGES {
        let path = corpus_root.join(rel);
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let cleaned = clean_page(&raw);
        if cleaned.is_empty() {
            continue;
        }
        let slug = Path::new(rel)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        pages.push((slug, cleaned));
    }
    let pages_in = pages.len();

    let mut out = String::with_capacity(64_000);
    out.push_str("# Principles runtime kernel (your default mind)\n\n");
    out.push_str(&format!(
        "<!-- distilled from {pages_in} pages | input_hash={hash} -->\n\n"
    ));
    out.push_str(
        "This is the compact operating kernel you start every investment \
         conversation with. It is distilled from four corpus wiki pages: the \
         runtime kernel plus Warren Buffett, Charlie Munger, and Ray Dalio \
         entity pages. Use it as the default reasoning protocol; use \
         `corpus_search` when the task needs a specific concept page, source \
         quote, knowledge page, company fact, or current-world evidence.\n\n---\n\n",
    );
    for (slug, body) in &pages {
        let title = slug.replace('-', " ");
        out.push_str(&format!("## {title}\n\n{body}\n\n---\n\n"));
    }

    let bytes_out = out.len();
    Ok((
        out,
        DistillReport {
            pages_in,
            bytes_out,
            input_hash: hash,
        },
    ))
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
