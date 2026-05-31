//! `corpus_read` — read a full local corpus wiki/source document by id.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::{corpus_search, ToolContext, ToolHandler};

pub const TOOL_NAME: &str = "corpus_read";
const DEFAULT_MAX_CHARS: usize = 20_000;
const HARD_MAX_CHARS: usize = 60_000;

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
            description: "Read a local LEEK investment corpus / 投研智库 document by id after `corpus_search` returns a relevant hit. Use this when a snippet is not enough and the task needs the full reasoning lens, source context, or exact page wording. Accepts corpus ids such as `wikis/principles/concepts/margin-of-safety` or `sources/principles/buffett/letters/1993`. This reads only the embedded local corpus; use `web_fetch` for external URLs."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Corpus document id returned by corpus_search, without .md suffix."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional cap on returned body characters (default 20000, hard max 60000)."
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
        _ctx: &ToolContext,
    ) -> Result<String> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'id' argument"))?
            .trim()
            .trim_end_matches(".md");
        if id.is_empty() {
            return Err(anyhow!("empty corpus document id"));
        }

        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(HARD_MAX_CHARS))
            .unwrap_or(DEFAULT_MAX_CHARS);

        let doc = corpus_search::lookup_doc(id)
            .ok_or_else(|| anyhow!("corpus document not found: {id}"))?;
        let (body, truncated) = truncate_chars(&doc.body, max_chars);
        let tags = if doc.tags.is_empty() {
            "(none)".to_string()
        } else {
            doc.tags.join(", ")
        };

        let mut out = format!(
            "# {title}\n\n- id: `{id}`\n- tier: `{tier}`\n- layer: `{layer}`\n- tags: {tags}\n\n{body}",
            title = doc.title,
            id = doc.id,
            tier = doc.tier,
            layer = doc.layer,
            tags = tags,
            body = body,
        );
        if truncated {
            out.push_str("\n\n[corpus_read: content truncated; call again with a narrower page or higher max_chars if the missing section is required.]");
        }
        Ok(out)
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> (&str, bool) {
    if text.chars().count() <= max_chars {
        return (text, false);
    }
    let cutoff = text
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len());
    (&text[..cutoff], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ctx() -> ToolContext {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ToolContext {
            pool,
            event_bus: crate::events::EventBus::new(),
            user_id: "u".to_string(),
            session_id: "s".to_string(),
            task_id: None,
        }
    }

    #[tokio::test]
    async fn reads_corpus_doc_by_id() {
        let tool = CorpusReadTool::new();
        let out = tool
            .call(
                serde_json::json!({
                    "id": "wikis/principles/concepts/margin-of-safety",
                    "max_chars": 2000
                }),
                CancellationToken::new(),
                &ctx().await,
            )
            .await
            .unwrap();
        assert!(out.contains("# Margin of Safety"));
        assert!(out.contains("wikis/principles/concepts/margin-of-safety"));
    }

    #[tokio::test]
    async fn unknown_doc_errors() {
        let tool = CorpusReadTool::new();
        let err = tool
            .call(
                serde_json::json!({"id": "wikis/principles/concepts/nope"}),
                CancellationToken::new(),
                &ctx().await,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("corpus document not found"));
    }
}
