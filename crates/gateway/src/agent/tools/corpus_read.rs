//! `corpus_read` — fetch one corpus document by its id (M2.2). The id
//! is the same string `corpus_search` returns: the POSIX-style relative
//! path from the corpus root, e.g. `sources/principles/munger/circle.md`.
//!
//! The model gets the full body for reasoning (corpus pages are bounded
//! by a 250-line lint cap — there is no further truncation at this
//! layer). The canvas card shows title + preview + byte count; the
//! expand view shows the full body and frontmatter.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::corpus::Corpus;
use crate::llm::ToolSpec;

const PREVIEW_CHARS: usize = 400;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "corpus_read".into(),
        description: "Read one document from leek's corpus by its id (the POSIX-style \
             relative path returned by `corpus_search`, e.g. \
             'sources/principles/munger/circle.md'). Returns the FULL body for \
             reasoning. Use after `corpus_search` surfaces a promising hit and you \
             need the document's full text or frontmatter."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Document id — the POSIX-style relative path \
                        returned by corpus_search. No leading slash."
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "读取知识库",
        result: ResultArtifact::Card("corpus_read"),
        summary: |args| {
            let id = args
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("读取知识库 · {}", super::summary_snippet(id))
        },
    }
}

pub fn run(corpus: &Arc<Corpus>, args: &serde_json::Value) -> ToolOutcome {
    let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("corpus_read: missing required string argument 'id'.");
    };
    let Some(doc) = corpus.read(id) else {
        return ToolOutcome::error(format!(
            "corpus_read: no document with id '{id}'. Use corpus_search first to \
             discover valid ids."
        ));
    };

    let body_bytes = doc.body.len();
    let body_preview: String = doc
        .body
        .chars()
        .take(PREVIEW_CHARS)
        .collect::<String>();
    let frontmatter_json: serde_json::Map<String, serde_json::Value> = doc
        .frontmatter
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    ToolOutcome::ok(
        // Model gets the full body — corpus pages are short enough (the
        // corpus lint caps wiki pages at ~250 lines) that there is no
        // need for a context-saving truncation layer here.
        doc.body.to_string(),
        serde_json::json!({
            "kind": "corpus_read",
            "id": doc.id,
            "title": doc.title,
            "body_preview": body_preview,
            // Full body lives in the payload so the canvas card can show
            // it on expand without a follow-up fetch — corpus pages are
            // lint-capped at ~250 lines, so payload bloat is bounded.
            "body": doc.body,
            "body_bytes": body_bytes,
            "frontmatter": frontmatter_json,
        }),
        serde_json::json!({
            "id": doc.id,
            "frontmatter": serde_json::Value::Object(
                doc.frontmatter
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            ),
            "body_bytes": body_bytes,
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
    fn known_id_returns_body() {
        let c = fixtures_corpus();
        let out = run(
            &c,
            &serde_json::json!({ "id": "sources/principles/munger/circle.md" }),
        );
        assert!(!out.is_error);
        assert_eq!(out.display_payload["kind"], "corpus_read");
        assert_eq!(
            out.display_payload["id"],
            "sources/principles/munger/circle.md"
        );
        // Body went to the model verbatim.
        assert!(out.model_output.contains("能力圈") || out.model_output.contains("circle"));
        // Card carries title + preview + byte count.
        assert!(out.display_payload["title"]
            .as_str()
            .unwrap_or("")
            .contains("能力圈"));
        assert!(out.display_payload["body_bytes"].as_u64().unwrap() > 0);
        // Frontmatter is structured, not a string.
        assert_eq!(
            out.display_payload["frontmatter"]["tier"],
            "principles"
        );
    }

    #[test]
    fn unknown_id_is_structured_error() {
        let c = fixtures_corpus();
        let out = run(&c, &serde_json::json!({ "id": "does/not/exist.md" }));
        assert!(out.is_error);
        assert!(out.model_output.contains("no document"));
    }

    #[test]
    fn missing_id_is_structured_error() {
        let c = fixtures_corpus();
        let out = run(&c, &serde_json::json!({}));
        assert!(out.is_error);
    }

    #[test]
    fn body_preview_caps_at_400_chars() {
        let c = fixtures_corpus();
        let out = run(
            &c,
            &serde_json::json!({ "id": "sources/principles/munger/circle.md" }),
        );
        let preview = out.display_payload["body_preview"].as_str().unwrap();
        assert!(preview.chars().count() <= PREVIEW_CHARS);
    }

    #[test]
    fn full_body_present_for_canvas_expand() {
        let c = fixtures_corpus();
        let out = run(
            &c,
            &serde_json::json!({ "id": "sources/principles/munger/circle.md" }),
        );
        // The card needs the full body to render the expanded view —
        // it must not have to round-trip back to the gateway.
        let body = out.display_payload["body"].as_str().unwrap();
        assert!(body.contains("能力圈"));
        assert!(body.len() >= out.display_payload["body_preview"].as_str().unwrap().len());
    }
}
