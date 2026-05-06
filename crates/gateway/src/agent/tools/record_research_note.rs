use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::events::EventEnvelope;
use crate::llm::ToolSpec;
use crate::vault::events as vault_events;

use super::{ToolContext, ToolHandler};

const TOOL_NAME: &str = "record_research_note";

pub struct RecordResearchNoteTool;

impl RecordResearchNoteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for RecordResearchNoteTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Record a reversible research note, mandate candidate, user preference, \
                 or reusable lesson in the vault artifacts table. Use this when the user \
                 says 'remember', 'note this', 'this fits my taste', or asks to preserve \
                 an insight. Do NOT use record_investment_action unless the user clearly \
                 commits capital to a specific ticker and direction."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "note_type": {
                        "type": "string",
                        "enum": ["research_note", "mandate_candidate", "preference", "lesson"],
                        "description": "The kind of note to save"
                    },
                    "title": {
                        "type": "string",
                        "description": "Short human-readable title"
                    },
                    "body": {
                        "type": "string",
                        "description": "Markdown body. Preserve the user's meaning; separate facts from judgment when relevant."
                    },
                    "corpus_refs": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional corpus wiki/source ids that support this note"
                    }
                },
                "required": ["note_type", "title", "body"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let note_type = required_str(&args, "note_type")?;
        let title = required_str(&args, "title")?;
        let body = required_str(&args, "body")?;
        let corpus_refs = args
            .get("corpus_refs")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let artifact_id = Uuid::now_v7().to_string();
        let now = Utc::now().to_rfc3339();
        let payload = serde_json::json!({
            "note_type": note_type,
            "title": title,
            "body": body,
            "corpus_refs": corpus_refs,
            "session_id": ctx.session_id,
            "task_id": ctx.task_id,
        });

        sqlx::query(
            r#"
            INSERT INTO artifacts
              (user_id, id, session_id, task_id, kind, payload_json, pinned, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?)
            "#,
        )
        .bind(&ctx.user_id)
        .bind(&artifact_id)
        .bind(&ctx.session_id)
        .bind(ctx.task_id.as_deref())
        .bind(&note_type)
        .bind(payload.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&ctx.pool)
        .await
        .context("inserting research note artifact")?;

        let event_payload = serde_json::json!({
            "artifact_id": artifact_id,
            "note_type": note_type,
            "title": title,
        });
        let ts = Utc::now();
        let evt_seq = vault_events::insert(
            &ctx.pool,
            &ctx.user_id,
            &ctx.session_id,
            ctx.task_id.as_deref(),
            "research_note_recorded",
            &event_payload,
            Some("agent"),
            ts,
        )
        .await
        .unwrap_or(0);
        ctx.event_bus
            .publish(
                &ctx.session_id,
                EventEnvelope {
                    seq: evt_seq,
                    kind: "research_note_recorded".to_string(),
                    payload: event_payload,
                    ts,
                },
            )
            .await;

        Ok(format!(
            "已记录为 `{note_type}`：{title}\n\nartifact_id: `{artifact_id}`"
        ))
    }
}

fn required_str(args: &serde_json::Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing '{key}' argument"))
}
