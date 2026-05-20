//! Walk a corpus root, parse frontmatter, build `DocumentInner` records.
//!
//! Skips: any dotfile / dotdir (catches `.git`, `.obsidian`, `.DS_Store`,
//! `.gitignore`) and any directory literally named `tools`. **Includes**
//! `_meta/` — taxonomy + protocols are search-relevant (ARCHITECTURE §8,
//! corpus/AGENTS.md, dispatch spec).
//!
//! `Document<'a>` is the public, borrow-only view (callers read
//! frontmatter + body without owning a copy); `DocumentInner` is what
//! lives in the index.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct DocumentInner {
    pub id: String,
    pub title: String,
    pub frontmatter: HashMap<String, String>,
    pub body: String,
}

/// Public, borrow-only view of a document — `Corpus::read` returns this.
#[derive(Debug, Clone, Copy)]
pub struct Document<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub frontmatter: &'a HashMap<String, String>,
    pub body: &'a str,
}

impl<'a> From<&'a DocumentInner> for Document<'a> {
    fn from(d: &'a DocumentInner) -> Self {
        Self {
            id: &d.id,
            title: &d.title,
            frontmatter: &d.frontmatter,
            body: &d.body,
        }
    }
}

/// One BM25 search result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}

/// Walk `root` recursively, parse every `*.md` we find, return the
/// `DocumentInner` list in stable `id`-ascending order. A bad single file
/// gets a `warn` and is skipped; only an unreadable root is an error.
pub fn walk_and_parse(root: &Path) -> Result<Vec<DocumentInner>> {
    let mut out = Vec::new();
    visit_dir(root, root, &mut out)?;
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn visit_dir(root: &Path, dir: &Path, out: &mut Vec<DocumentInner>) -> Result<()> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, dir = %dir.display(), "corpus: cannot read dir");
            return Ok(());
        }
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Dotfiles / dotdirs: covers `.git`, `.obsidian`, `.DS_Store`, …
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            // `tools/` holds enforcer scripts (corpus/AGENTS.md), never
            // search material — skip at any depth.
            if name == "tools" {
                continue;
            }
            visit_dir(root, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            match parse_file(root, &path) {
                Ok(doc) => out.push(doc),
                Err(e) => {
                    tracing::warn!(error = %e, file = %path.display(), "corpus: skip file");
                }
            }
        }
    }
    Ok(())
}

fn parse_file(root: &Path, path: &Path) -> Result<DocumentInner> {
    let raw = std::fs::read_to_string(path)?;
    let (frontmatter, body_str) = split_frontmatter(&raw);
    let body = body_str.to_string();
    let id = relative_id(root, path);

    // title precedence: frontmatter.title → first `#` heading → file stem.
    let title = frontmatter
        .get("title")
        .cloned()
        .or_else(|| first_heading(&body))
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| id.clone())
        });

    Ok(DocumentInner {
        id,
        title,
        frontmatter,
        body,
    })
}

/// POSIX-style path relative to `root` (`sources/principles/...`).
/// `\` → `/` so Windows callers see the same ids as POSIX ones.
fn relative_id(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

fn first_heading(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim_start)
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
}

/// Tiny YAML-lite parser. Flat `key: value` and `key: [a, b, c]` only —
/// no nested objects, no folded scalars, no block strings. A malformed
/// frontmatter block yields an empty map: the loader prefers graceful
/// degradation over hard failure (dispatch spec line 56–58).
fn split_frontmatter(input: &str) -> (HashMap<String, String>, &str) {
    let after_open = if let Some(rest) = input.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = input.strip_prefix("---\r\n") {
        rest
    } else {
        return (HashMap::new(), input);
    };
    // Find a `---` that occupies its own line.
    let close = after_open.match_indices("---").find(|(i, _)| {
        let at_line_start =
            *i == 0 || after_open.as_bytes().get(i.wrapping_sub(1)) == Some(&b'\n');
        let after = &after_open[*i + 3..];
        let at_line_end =
            after.is_empty() || after.starts_with('\n') || after.starts_with('\r');
        at_line_start && at_line_end
    });
    let Some((close_idx, _)) = close else {
        // Open delimiter without a close — treat as no frontmatter.
        return (HashMap::new(), input);
    };
    let yaml = &after_open[..close_idx];
    let body_rest = &after_open[close_idx + 3..];
    let body = body_rest
        .trim_start_matches('\r')
        .trim_start_matches('\n');

    let mut map = HashMap::new();
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once(':') else {
            continue;
        };
        let key = k.trim().to_string();
        let mut val = v.trim().to_string();
        // Strip matched quotes.
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = val[1..val.len() - 1].to_string();
        } else if val.starts_with('[') && val.ends_with(']') && val.len() >= 2 {
            // Flat array: keep inner contents as a comma-joined string so
            // the HashMap<String, String> contract holds.
            val = val[1..val.len() - 1].trim().to_string();
        }
        map.insert(key, val);
    }
    (map, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_round_trip() {
        let input = "---\ntitle: Test\ntags: [a, b]\n---\n\nBody line 1\nBody line 2\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm.get("title").map(|s| s.as_str()), Some("Test"));
        assert_eq!(fm.get("tags").map(|s| s.as_str()), Some("a, b"));
        assert_eq!(body, "Body line 1\nBody line 2\n");
    }

    #[test]
    fn frontmatter_open_without_close_falls_back() {
        let input = "---\nnot really frontmatter\nbody continues";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_empty());
        assert_eq!(body, input);
    }

    #[test]
    fn frontmatter_absent_keeps_body() {
        let input = "# Just a heading\nand a body";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_empty());
        assert_eq!(body, input);
    }

    #[test]
    fn frontmatter_quoted_value_stripped() {
        let input = "---\ntitle: \"With spaces\"\n---\nbody";
        let (fm, _) = split_frontmatter(input);
        assert_eq!(fm.get("title").map(|s| s.as_str()), Some("With spaces"));
    }

    #[test]
    fn first_heading_finds_h1_only() {
        assert_eq!(first_heading("# Hello\nrest"), Some("Hello".to_string()));
        assert_eq!(first_heading("body\n# Hello"), Some("Hello".to_string()));
        assert_eq!(first_heading("## subheading"), None);
        assert_eq!(first_heading("no heading"), None);
    }
}
