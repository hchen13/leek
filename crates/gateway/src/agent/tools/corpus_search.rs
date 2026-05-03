//! `corpus_search` — keyword search over the embedded LEEK corpus.
//!
//! Reads every `.md` under `corpus/wikis/` and `corpus/sources/` (embedded at
//! compile time via `rust_embed`), parses YAML frontmatter, and supports
//! case-insensitive substring scoring with optional tier/layer/tag filters.
//!
//! P3 design notes:
//! - No embeddings, no BM25 — flat substring scoring, ordered by hit-count
//!   then by node degree (higher = more central in the corpus graph).
//! - Returns top-N hits with a 240-char snippet around the first match so
//!   the model sees enough context to decide whether to web_fetch / drill in.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rust_embed::RustEmbed;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "corpus_search";
const MAX_LIMIT: usize = 20;
const DEFAULT_LIMIT: usize = 5;
const SNIPPET_HALF_WIDTH: usize = 120;

#[derive(RustEmbed)]
#[folder = "../../corpus"]
#[include = "wikis/**/*.md"]
#[include = "sources/**/*.md"]
struct CorpusEmbed;

#[derive(Debug, Clone)]
struct CorpusDoc {
    /// Wikilink-style id (no `.md` suffix, e.g. `wikis/principles/concepts/margin-of-safety`).
    id: String,
    title: String,
    tier: String,  // "principles" | "knowledge"
    layer: String, // "wiki" | "source"
    tags: Vec<String>,
    body: String,
    /// Lowercased haystack — body + title + tags concatenated.
    /// Stored once to avoid re-lowercasing on every search.
    haystack: String,
}

fn corpus_docs() -> &'static [CorpusDoc] {
    static DOCS: OnceLock<Vec<CorpusDoc>> = OnceLock::new();
    DOCS.get_or_init(load_corpus)
}

/// Read-only view of a corpus doc, exposed so the HTTP API can serve full
/// document text (e.g. for the Brain widget's "click a node → see content"
/// preview). Body string is owned to decouple from the lazy static lifetime
/// at the trait boundary; the embedded data is not very large (~7MB total).
#[derive(serde::Serialize)]
pub struct CorpusDocView {
    pub id: String,
    pub title: String,
    pub tier: String,
    pub layer: String,
    pub tags: Vec<String>,
    pub body: String,
}

pub fn lookup_doc(id: &str) -> Option<CorpusDocView> {
    let d = corpus_docs().iter().find(|d| d.id == id)?;
    Some(CorpusDocView {
        id: d.id.clone(),
        title: d.title.clone(),
        tier: d.tier.clone(),
        layer: d.layer.clone(),
        tags: d.tags.clone(),
        body: d.body.clone(),
    })
}

fn load_corpus() -> Vec<CorpusDoc> {
    let mut docs = Vec::new();
    for path in CorpusEmbed::iter() {
        let Some(file) = CorpusEmbed::get(&path) else {
            continue;
        };
        let bytes = file.data.into_owned();
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        let Some((fm_raw, body)) = split_frontmatter(&content) else {
            continue;
        };
        let fm: Frontmatter = match serde_yaml::from_str(fm_raw) {
            Ok(fm) => fm,
            Err(_) => continue,
        };
        let Some(tier) = fm.tier else { continue };
        let Some(layer) = fm.layer else { continue };

        let id_path = path.trim_end_matches(".md");
        let id = id_path.replace('\\', "/");
        let title = fm.title.unwrap_or_else(|| id.clone());

        let mut haystack = String::with_capacity(body.len() + 256);
        haystack.push_str(&title.to_lowercase());
        haystack.push('\n');
        for t in &fm.tags {
            haystack.push_str(&t.to_lowercase());
            haystack.push(' ');
        }
        haystack.push('\n');
        haystack.push_str(&body.to_lowercase());

        docs.push(CorpusDoc {
            id,
            title,
            tier,
            layer,
            tags: fm.tags,
            body: body.to_string(),
            haystack,
        });
    }
    tracing::info!(loaded = docs.len(), "corpus_search loaded embedded corpus");
    docs
}

#[derive(serde::Deserialize, Default)]
struct Frontmatter {
    title: Option<String>,
    tier: Option<String>,
    layer: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    if !content.starts_with("---") {
        return None;
    }
    let after = content.get(3..)?.strip_prefix('\n').unwrap_or(content.get(3..)?);
    let end = after.find("\n---")?;
    let fm = &after[..end];
    let rest = after.get(end + 4..)?;
    Some((fm, rest.trim_start_matches('\n')))
}

pub struct CorpusSearchTool;

impl CorpusSearchTool {
    pub fn new() -> Self {
        // Force eager load so first user query doesn't pay the parse cost.
        let _ = corpus_docs();
        Self
    }
}

#[async_trait]
impl ToolHandler for CorpusSearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Search the local LEEK corpus (curated investing wikis + source texts: \
                 Buffett letters, Dalio books, Munger speeches, NVIDIA/AI/macro notes, etc.). \
                 Use this BEFORE web_search when the question is about value-investing \
                 principles, mental models, or the people / books you'd find in a serious \
                 investor's library — the corpus is hand-curated and more authoritative \
                 than the open web for those topics. Returns top hits as JSON with id, \
                 title, tier, layer, tags, and a snippet around the first match. Combine \
                 with web_fetch (using the returned id as a wikilink reference) to read \
                 the full document if needed."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Keywords to match (case-insensitive substring; whitespace-separated terms are AND-ed)."
                    },
                    "tier": {
                        "type": "string",
                        "enum": ["principles", "knowledge"],
                        "description": "Optional tier filter. principles = durable mental models / canonical investors. knowledge = current macro / industry / company notes."
                    },
                    "layer": {
                        "type": "string",
                        "enum": ["wiki", "source"],
                        "description": "Optional layer filter. wiki = synthesized concept page. source = primary source text."
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional tag filter — only docs tagged with ALL listed tags are returned."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max hits to return (default 5, max 20)."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'query' argument"))?
            .trim()
            .to_string();
        if query.is_empty() {
            return Ok(serde_json::json!({"hits": [], "note": "empty query"}).to_string());
        }
        let tier_filter = args.get("tier").and_then(|v| v.as_str()).map(String::from);
        let layer_filter = args.get("layer").and_then(|v| v.as_str()).map(String::from);
        let tag_filter: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(|s| s.to_lowercase()))
                    .collect()
            })
            .unwrap_or_default();
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(MAX_LIMIT).max(1))
            .unwrap_or(DEFAULT_LIMIT);

        let terms: Vec<String> = query
            .split_whitespace()
            .map(|s| s.to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if terms.is_empty() {
            return Ok(serde_json::json!({"hits": []}).to_string());
        }

        let docs = corpus_docs();
        let mut scored: Vec<(usize, &CorpusDoc, usize)> = docs
            .iter()
            .filter(|d| tier_filter.as_deref().map_or(true, |t| d.tier == t))
            .filter(|d| layer_filter.as_deref().map_or(true, |l| d.layer == l))
            .filter(|d| {
                if tag_filter.is_empty() {
                    return true;
                }
                let lower_tags: Vec<String> = d.tags.iter().map(|t| t.to_lowercase()).collect();
                tag_filter.iter().all(|t| lower_tags.contains(t))
            })
            .filter_map(|d| {
                // AND across terms — every term must match somewhere in haystack.
                let mut hits = 0usize;
                let mut first_pos: Option<usize> = None;
                for term in &terms {
                    let count = d.haystack.matches(term.as_str()).count();
                    if count == 0 {
                        return None;
                    }
                    hits += count;
                    if first_pos.is_none() {
                        first_pos = d.haystack.find(term.as_str());
                    }
                }
                Some((hits, d, first_pos.unwrap_or(0)))
            })
            .collect();

        // Order: hit count desc, then title length asc (prefer concise titles).
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.len().cmp(&b.1.title.len())));
        scored.truncate(limit);

        let hits: Vec<_> = scored
            .into_iter()
            .map(|(score, d, pos)| {
                let snippet = build_snippet(&d.body, pos);
                serde_json::json!({
                    "id": d.id,
                    "title": d.title,
                    "tier": d.tier,
                    "layer": d.layer,
                    "tags": d.tags,
                    "score": score,
                    "snippet": snippet,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "query": query,
            "filters": {
                "tier": tier_filter,
                "layer": layer_filter,
                "tags": tag_filter,
            },
            "total_corpus_docs": docs.len(),
            "hits": hits,
        })
        .to_string())
    }
}

/// Pull a ~240-char window around `pos` (which is a byte offset into the
/// lowercased haystack — body alignment is approximate but good enough for
/// snippet display). Snaps to char boundaries to stay UTF-8 valid.
fn build_snippet(body: &str, pos: usize) -> String {
    if body.is_empty() {
        return String::new();
    }
    let pos = pos.min(body.len());
    let start = pos.saturating_sub(SNIPPET_HALF_WIDTH);
    let end = (pos + SNIPPET_HALF_WIDTH).min(body.len());
    // Snap to char boundaries.
    let start = (0..=start).rev().find(|i| body.is_char_boundary(*i)).unwrap_or(0);
    let end = (end..=body.len())
        .find(|i| body.is_char_boundary(*i))
        .unwrap_or(body.len());
    let mut snippet = body[start..end].replace('\n', " ");
    while snippet.contains("  ") {
        snippet = snippet.replace("  ", " ");
    }
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < body.len() { "…" } else { "" };
    format!("{prefix}{}{suffix}", snippet.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_loads_at_least_some_docs() {
        // Sanity: build.rs / RustEmbed should have pulled in the wikis/sources tree.
        let docs = corpus_docs();
        assert!(
            !docs.is_empty(),
            "expected at least one corpus doc to be embedded"
        );
        // Spot-check structure on a known anchor.
        assert!(
            docs.iter()
                .any(|d| d.id.contains("margin-of-safety") || d.title.to_lowercase().contains("margin")),
            "expected margin-of-safety in corpus"
        );
    }

    async fn make_ctx() -> super::ToolContext {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        super::ToolContext {
            pool,
            event_bus: crate::events::EventBus::new(),
            user_id: "test".into(),
            session_id: "sess".into(),
            task_id: None,
        }
    }

    #[tokio::test]
    async fn search_returns_relevant_hit() {
        let ctx = make_ctx().await;
        let tool = CorpusSearchTool::new();
        let out = tool
            .call(
                serde_json::json!({"query": "margin of safety", "limit": 3}),
                CancellationToken::new(),
                &ctx,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let hits = v["hits"].as_array().unwrap();
        assert!(!hits.is_empty(), "expected at least one hit for 'margin of safety'");
        let first = &hits[0];
        assert!(first["title"].as_str().is_some());
        assert!(first["snippet"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn search_respects_tier_filter() {
        let ctx = make_ctx().await;
        let tool = CorpusSearchTool::new();
        let out = tool
            .call(
                serde_json::json!({
                    "query": "the",
                    "tier": "knowledge",
                    "limit": 5
                }),
                CancellationToken::new(),
                &ctx,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        for hit in v["hits"].as_array().unwrap() {
            assert_eq!(hit["tier"].as_str().unwrap(), "knowledge");
        }
    }

    #[tokio::test]
    async fn search_empty_query_returns_empty_hits() {
        let ctx = make_ctx().await;
        let tool = CorpusSearchTool::new();
        let out = tool
            .call(
                serde_json::json!({"query": "  "}),
                CancellationToken::new(),
                &ctx,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hits"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn snippet_truncates_with_ellipsis() {
        let body = "a".repeat(500);
        let snip = build_snippet(&body, 250);
        assert!(snip.starts_with('…'));
        assert!(snip.ends_with('…'));
        // Body length 500, 250 + 120 = 370 < 500 → suffix … should be present.
    }
}
