use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use walkdir::WalkDir;

use super::{CorpusEdge, CorpusGraph, CorpusNode};

#[derive(Debug, Deserialize, Default)]
struct Frontmatter {
    title: Option<String>,
    slug: Option<String>,
    layer: Option<String>,
    tier: Option<String>,
    #[serde(rename = "type")]
    node_type: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    sources: Vec<String>,
}

pub fn build_graph(corpus_root: &Path) -> Result<CorpusGraph> {
    let root = corpus_root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", corpus_root.display()))?;

    let mut nodes: HashMap<String, CorpusNode> = HashMap::new();
    let mut raw_edges: Vec<CorpusEdge> = Vec::new();

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| !is_excluded_dir(e.path(), &root))
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let Some(rel) = path.strip_prefix(&root).ok().map(PathBuf::from) else {
            continue;
        };
        // Wikilink id: drop the .md extension and normalize to forward slashes.
        let id_path = rel.with_extension("");
        let id = id_path.to_string_lossy().replace('\\', "/");

        if !is_corpus_page(&id) {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skip unreadable corpus file");
                continue;
            }
        };

        let (fm_raw, body) = match split_frontmatter(&content) {
            Some(parts) => parts,
            None => {
                tracing::warn!(path = %path.display(), "missing frontmatter");
                continue;
            }
        };

        let fm: Frontmatter = match serde_yaml::from_str(fm_raw) {
            Ok(fm) => fm,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "invalid frontmatter YAML");
                continue;
            }
        };

        let Some(layer) = fm.layer.as_deref() else {
            tracing::warn!(path = %path.display(), "frontmatter missing `layer`");
            continue;
        };
        let Some(tier) = fm.tier.as_deref() else {
            tracing::warn!(path = %path.display(), "frontmatter missing `tier`");
            continue;
        };

        let Some(cluster) = cluster_for(tier, layer) else {
            tracing::warn!(
                path = %path.display(),
                tier = tier,
                layer = layer,
                "unknown tier/layer combo"
            );
            continue;
        };

        nodes.insert(
            id.clone(),
            CorpusNode {
                id: id.clone(),
                cluster: cluster.to_string(),
                title: fm.title.clone().unwrap_or_else(|| id.clone()),
                slug: fm.slug.clone().unwrap_or_default(),
                node_type: fm.node_type.clone(),
                tier: tier.to_string(),
                layer: layer.to_string(),
                tags: fm.tags.clone(),
                degree: 0,
            },
        );

        // Wikilink edges
        for cap in wikilink_re().captures_iter(body) {
            let target = &cap[1];
            let target = target.split('|').next().unwrap_or(target).trim();
            // Strip a trailing `.md` if anyone wrote one (unconventional but safe)
            let target = target.trim_end_matches(".md");
            raw_edges.push(CorpusEdge {
                from: id.clone(),
                to: target.to_string(),
                kind: "wikilink".into(),
            });
        }

        // Source-ref edges from frontmatter
        for src in &fm.sources {
            let target = src.trim().trim_end_matches(".md");
            raw_edges.push(CorpusEdge {
                from: id.clone(),
                to: target.to_string(),
                kind: "source-ref".into(),
            });
        }
    }

    // Deduplicate (from,to,kind) — wikilinks repeated in body would double up.
    let mut seen = HashSet::new();
    let mut edges: Vec<CorpusEdge> = Vec::with_capacity(raw_edges.len());
    for e in raw_edges {
        let key = (e.from.clone(), e.to.clone(), e.kind.clone());
        if seen.insert(key) {
            edges.push(e);
        }
    }

    // Drop edges pointing to nodes we don't have (broken links).
    let valid: HashSet<&String> = nodes.keys().collect();
    let dropped = edges.len();
    edges.retain(|e| valid.contains(&e.to));
    let dropped = dropped - edges.len();
    if dropped > 0 {
        tracing::info!(dropped, "broken wikilink edges dropped");
    }

    // Compute degree (sum of in + out).
    for edge in &edges {
        if let Some(n) = nodes.get_mut(&edge.from) {
            n.degree += 1;
        }
        if let Some(n) = nodes.get_mut(&edge.to) {
            n.degree += 1;
        }
    }

    let mut nodes_vec: Vec<CorpusNode> = nodes.into_values().collect();
    nodes_vec.sort_by(|a, b| a.id.cmp(&b.id));
    edges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));

    Ok(CorpusGraph {
        version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        corpus_commit: read_git_commit(&root),
        nodes: nodes_vec,
        edges,
    })
}

fn cluster_for(tier: &str, layer: &str) -> Option<&'static str> {
    match (tier, layer) {
        ("principles", "wiki") => Some("principles-wikis"),
        ("principles", "source") => Some("principles-sources"),
        ("knowledge", "wiki") => Some("knowledge-wikis"),
        ("knowledge", "source") => Some("knowledge-sources"),
        _ => None,
    }
}

fn is_corpus_page(id: &str) -> bool {
    if id.starts_with("_meta/") || id.starts_with("tools/") {
        return false;
    }
    if id == "AGENTS" || id == "README" {
        return false;
    }
    id.starts_with("wikis/") || id.starts_with("sources/")
}

fn is_excluded_dir(path: &Path, root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let s = rel.to_string_lossy();
    s.starts_with("_meta")
        || s.starts_with("tools")
        || s.starts_with(".git")
        || s.starts_with(".obsidian")
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    if !content.starts_with("---") {
        return None;
    }
    let after = content.get(3..)?;
    let after = after.strip_prefix('\n').unwrap_or(after);
    // Look for the closing `---` on its own line.
    let end = after.find("\n---")?;
    let fm = &after[..end];
    let rest = after.get(end + 4..)?;
    Some((fm, rest.trim_start_matches('\n')))
}

fn wikilink_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\[\]]+)\]\]").unwrap())
}

fn read_git_commit(root: &Path) -> Option<String> {
    let head_path = root.join(".git/HEAD");
    let head = fs::read_to_string(&head_path).ok()?;
    if let Some(ref_path) = head.strip_prefix("ref: ").map(|s| s.trim().to_string()) {
        let ref_file = root.join(".git").join(ref_path);
        let commit = fs::read_to_string(ref_file).ok()?;
        return Some(commit.trim().to_string());
    }
    // Detached HEAD: HEAD itself is the commit
    Some(head.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let content = "---\ntitle: Foo\n---\nbody text";
        let (fm, body) = split_frontmatter(content).unwrap();
        assert_eq!(fm.trim(), "title: Foo");
        assert_eq!(body, "body text");
    }

    #[test]
    fn split_frontmatter_missing() {
        assert!(split_frontmatter("no frontmatter").is_none());
    }

    #[test]
    fn cluster_combos() {
        assert_eq!(cluster_for("principles", "wiki"), Some("principles-wikis"));
        assert_eq!(cluster_for("knowledge", "source"), Some("knowledge-sources"));
        assert_eq!(cluster_for("foo", "bar"), None);
    }

    #[test]
    fn wikilink_extraction() {
        let body = "see [[wikis/principles/concepts/foo]] and [[sources/x|alias]] also [[bar]]";
        let caps: Vec<String> = wikilink_re()
            .captures_iter(body)
            .map(|c| c[1].to_string())
            .collect();
        assert_eq!(
            caps,
            vec![
                "wikis/principles/concepts/foo",
                "sources/x|alias",
                "bar",
            ]
        );
    }

    #[test]
    fn excluded_dirs() {
        let root = Path::new("/tmp/corpus");
        assert!(is_excluded_dir(&root.join("_meta/index.md"), root));
        assert!(is_excluded_dir(&root.join("tools/lint.py"), root));
        assert!(is_excluded_dir(&root.join(".git/HEAD"), root));
        assert!(!is_excluded_dir(&root.join("wikis/principles"), root));
    }
}
