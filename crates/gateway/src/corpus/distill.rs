//! Distill `corpus/wikis/principles/{concepts,entities}/*.md` into a single
//! markdown blob suitable for system-prompt embedding.
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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

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

fn clean_page(raw: &str) -> String {
    let mut text = strip_frontmatter(raw).trim_start().to_string();
    for heading in [
        // path-only scaffolding (no semantic content for the LLM)
        "## 来源",
        "## Sources",
        "## 相关概念",
        "## Related Concepts",
        "## Related",
        // background / case material — heavy and better served by
        // corpus_search at runtime when the conversation actually needs it.
        // We keep 定义 / 核心要义 / 实践应用 / 常见误区 (the operational core).
        "## 思想演变",
        "## Evolution of Thought",
        "## 典型案例",
        "## Canonical Cases",
        "## 开放问题",
        "## Open Questions",
    ] {
        text = strip_section(&text, heading);
    }
    simplify_wikilinks(&text).trim().to_string()
}

/// Stable content hash over `wikis/principles/**/*.md` (paths sorted, content
/// concatenated with NUL separators). Used to detect drift between distilled
/// output and the corpus it was distilled from.
fn input_hash(principles_dir: &Path) -> Result<String> {
    let mut paths: Vec<_> = WalkDir::new(principles_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .map(|e| e.into_path())
        .collect();
    paths.sort();

    let mut hasher = Sha256::new();
    for path in paths {
        let rel = path.strip_prefix(principles_dir).unwrap_or(&path);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(b"\0");
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

/// Distill the principles tier of a corpus into a single embeddable markdown
/// blob. Pages from `wikis/principles/concepts/` and `wikis/principles/entities/`
/// are sorted by slug for reproducibility.
pub fn distill(corpus_root: &Path) -> Result<(String, DistillReport)> {
    let principles_dir = corpus_root.join("wikis").join("principles");
    if !principles_dir.exists() {
        anyhow::bail!("expected wikis/principles/ at {}", principles_dir.display());
    }

    let hash = input_hash(&principles_dir)?;

    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    for entry in WalkDir::new(&principles_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
    {
        let raw = fs::read_to_string(entry.path())
            .with_context(|| format!("read {}", entry.path().display()))?;
        let cleaned = clean_page(&raw);
        if cleaned.is_empty() {
            continue;
        }
        let slug = entry
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        pages.insert(slug, cleaned);
    }
    let pages_in = pages.len();

    let mut out = String::with_capacity(200_000);
    out.push_str("# Investing principles (your default mind)\n\n");
    out.push_str(&format!(
        "<!-- distilled from {pages_in} pages | input_hash={hash} -->\n\n"
    ));
    out.push_str(
        "This is the wisdom you start every conversation with — the thinking \
         framework, distilled from the corpus principles tier (Buffett, Munger, \
         Dalio, …). You don't need to `corpus_search` these; they're already in \
         your prompt. Use `corpus_search` for `wikis/knowledge/` (entities, \
         current state) and `sources/` (raw material).\n\n---\n\n",
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
