//! `corpus_search` — lexical (BM25) lookup into the in-process corpus
//! (M2.2). Hits go back to the model verbatim, and to the canvas as a
//! structured `corpus_search` card.
//!
//! The model gets a compact list (`title — id`, then snippet) for
//! reasoning; the card payload carries every hit's id / title / snippet /
//! score so the UI can render and the user can click through to
//! `corpus_read` for the full body.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::corpus::Corpus;
use crate::llm::ToolSpec;

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 25;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "corpus_search".into(),
        description: "Search leek's curated investing-wisdom corpus by keyword and \
             return the top hits (id, title, snippet, BM25 score). Use this BEFORE \
             pulling on the public web for any question that might be covered by \
             principles (Munger, Buffett, Dalio, …) or by previously curated \
             knowledge pages. The id returned here is the input to `corpus_read` \
             when you need the full document body. Lexical-only (no embeddings); \
             tokenization handles CJK per-character and Latin words on whitespace."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query — keywords or short phrase. \
                        Examples: 'circle of competence', 'NVDA GPU 市场', 'margin of safety'."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum hits to return (default 5, max 25).",
                    "minimum": 1,
                    "maximum": MAX_LIMIT
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "知识库搜索",
        result: ResultArtifact::Card("corpus_search"),
        summary: |args| {
            let q = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("知识库搜索 · {}", super::summary_snippet(q))
        },
    }
}

pub fn run(corpus: &Arc<Corpus>, args: &serde_json::Value) -> ToolOutcome {
    let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("corpus_search: missing required string argument 'query'.");
    };
    let query = query.trim();
    if query.is_empty() {
        return ToolOutcome::error("corpus_search: 'query' must be non-empty.");
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);

    let hits = corpus.search(query, limit);

    let model_output = if hits.is_empty() {
        format!("Found 0 hits for \"{query}\".")
    } else {
        let mut s = format!("Found {} hits for \"{}\":\n", hits.len(), query);
        for (i, h) in hits.iter().enumerate() {
            s.push_str(&format!(
                "{}. {} — {}\n   {}\n",
                i + 1,
                h.id,
                h.title,
                h.snippet
            ));
        }
        s
    };

    let hits_json: Vec<serde_json::Value> = hits
        .iter()
        .map(|h| {
            serde_json::json!({
                "id": h.id,
                "title": h.title,
                "snippet": h.snippet,
                "score": h.score,
            })
        })
        .collect();

    ToolOutcome::ok(
        model_output,
        // `kind` is redundant with `card_kind` on the artifact frame, but
        // dispatch spec line 119 names it explicitly — keep it so the
        // payload is self-describing for debug / archive consumers.
        serde_json::json!({
            "kind": "corpus_search",
            "query": query,
            "hits": hits_json,
        }),
        serde_json::json!({
            "query": query,
            "limit": limit,
            "total_docs": corpus.len(),
            "scores": hits.iter().map(|h| h.score).collect::<Vec<_>>(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures_corpus() -> Arc<Corpus> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
        Arc::new(Corpus::load(&root).unwrap())
    }

    #[test]
    fn returns_hits_for_known_term() {
        let c = fixtures_corpus();
        let out = run(&c, &serde_json::json!({ "query": "competence" }));
        assert!(!out.is_error);
        assert_eq!(out.display_payload["kind"], "corpus_search");
        let hits = out.display_payload["hits"].as_array().unwrap();
        assert!(!hits.is_empty());
        assert!(hits
            .iter()
            .any(|h| h["id"].as_str().unwrap_or("").contains("circle.md")));
    }

    #[test]
    fn empty_hits_for_unknown_term() {
        let c = fixtures_corpus();
        let out = run(
            &c,
            &serde_json::json!({ "query": "zzzNotAnywhereInCorpuszzz" }),
        );
        assert!(!out.is_error);
        let hits = out.display_payload["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 0);
        assert!(out.model_output.contains("Found 0 hits"));
    }

    #[test]
    fn limit_caps_returned_hits() {
        let c = fixtures_corpus();
        let out = run(
            &c,
            &serde_json::json!({ "query": "the", "limit": 1 }),
        );
        assert!(!out.is_error);
        let hits = out.display_payload["hits"].as_array().unwrap();
        assert!(hits.len() <= 1);
    }

    #[test]
    fn missing_query_is_structured_error() {
        let c = fixtures_corpus();
        let out = run(&c, &serde_json::json!({}));
        assert!(out.is_error);
    }

    #[test]
    fn empty_query_is_structured_error() {
        let c = fixtures_corpus();
        let out = run(&c, &serde_json::json!({ "query": "   " }));
        assert!(out.is_error);
    }
}
