//! `corpus_read` — fetch the full body of an embedded corpus document by id.
//!
//! `corpus_search` returns 240-char snippets; that's enough to decide whether
//! a hit is worth reading, but not enough to actually answer a question. The
//! agent used to reach for `web_fetch` with a corpus id like
//! `wikis/principles/concepts/margin-of-safety`, which then tried to GET it
//! as an HTTP URL and failed (or worse, hit something on the open web).
//! `corpus_read` is the local equivalent: take the id, return the full doc
//! (frontmatter + body) up to a bounded char cap.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::corpus_search::lookup_doc;
use super::ToolHandler;
use crate::llm::ToolSpec;

const TOOL_NAME: &str = "corpus_read";
const DEFAULT_MAX_CHARS: usize = 20_000;
const HARD_CAP_MAX_CHARS: usize = 50_000;

pub struct CorpusReadTool;

impl CorpusReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for CorpusReadTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Read the full body of a local LEEK corpus document by id. \
                Use AFTER `corpus_search` when a hit's snippet looks relevant and you \
                need the rest of the doc to answer (definitions, full quotes, longer \
                arguments, complete tables). Pass the wikilink-style id you got back \
                from corpus_search, e.g. `wikis/principles/concepts/margin-of-safety`. \
                Do NOT hand corpus ids to `web_fetch` — they are not URLs. Returns \
                title, tier, layer, tags, and the document body as markdown, truncated \
                to `max_chars` (default 20000, hard cap 50000)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Wikilink id from a corpus_search hit, e.g. 'wikis/principles/concepts/margin-of-safety'."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional cap on returned body length (default 20000, hard max 50000). Truncated output is suffixed with a notice.",
                        "minimum": 1
                    }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing 'id' argument"))?;
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(HARD_CAP_MAX_CHARS))
            .unwrap_or(DEFAULT_MAX_CHARS);

        let normalized = normalize_id(id);
        let doc = lookup_doc(&normalized).ok_or_else(|| {
            anyhow!(
                "no corpus document with id `{normalized}`. \
                 Search again via `corpus_search` and use the `id` field from a hit; \
                 corpus ids are not URLs."
            )
        })?;

        let (body, truncated) = truncate_body(&doc.body, max_chars);
        let payload = serde_json::json!({
            "id": doc.id,
            "title": doc.title,
            "tier": doc.tier,
            "layer": doc.layer,
            "tags": doc.tags,
            "body": body,
            "body_chars": doc.body.chars().count(),
            "truncated": truncated,
        });
        Ok(payload.to_string())
    }
}

/// Strip leading slashes and any `.md` suffix in case the model passes the id
/// in a slightly different shape. Corpus ids in `corpus_search` results are
/// returned without `.md`.
fn normalize_id(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('/')
        .trim_end_matches(".md")
        .to_string()
}

fn truncate_body(body: &str, max_chars: usize) -> (String, bool) {
    if body.chars().count() <= max_chars {
        return (body.to_string(), false);
    }
    let mut truncated: String = body.chars().take(max_chars).collect();
    truncated.push_str("\n\n[…corpus_read truncated; raise max_chars to read more]");
    (truncated, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_md_suffix_and_leading_slash() {
        assert_eq!(
            normalize_id("/wikis/principles/concepts/margin-of-safety.md"),
            "wikis/principles/concepts/margin-of-safety"
        );
    }

    #[tokio::test]
    async fn missing_id_returns_explanatory_error() {
        let tool = CorpusReadTool::new();
        let ctx = make_ctx().await;
        let err = tool
            .call(
                serde_json::json!({"id": "wikis/does-not-exist"}),
                CancellationToken::new(),
                &ctx,
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no corpus document"));
        assert!(msg.contains("not URLs") || msg.contains("not URL"));
    }

    #[tokio::test]
    async fn reads_known_principle_doc() {
        let tool = CorpusReadTool::new();
        let ctx = make_ctx().await;
        let raw = tool
            .call(
                serde_json::json!({"id": "wikis/principles/concepts/margin-of-safety"}),
                CancellationToken::new(),
                &ctx,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            v["id"].as_str().unwrap(),
            "wikis/principles/concepts/margin-of-safety"
        );
        assert!(v["body"].as_str().unwrap().len() > 0);
    }

    async fn make_ctx() -> super::super::ToolContext {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        super::super::ToolContext {
            pool,
            event_bus: crate::events::EventBus::new(),
            user_id: "test".into(),
            session_id: "sess".into(),
            task_id: None,
            tuning: crate::llm::LlmTuning::defaults(),
        }
    }
}
