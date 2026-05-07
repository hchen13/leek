use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::events::EventEnvelope;
use crate::llm::ToolSpec;
use crate::vault::{events as vault_events, plans as vault_plans};

use super::{ToolContext, ToolHandler};

const TOOL_NAME: &str = "update_plan";

pub struct UpdatePlanTool;

impl UpdatePlanTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for UpdatePlanTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Create or replace the active execution plan for the current research task. \
                Use this for non-trivial research before doing the work, then update it as items \
                move from pending to in_progress to completed. The runtime will not accept a final \
                task answer while the active plan still has pending or in_progress items."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "explanation": {
                        "type": "string",
                        "description": "Short reason for the plan update"
                    },
                    "plan": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Stable short id for this item, for example p1"
                                },
                                "step": {
                                    "type": "string",
                                    "description": "One concise, verifiable action"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"]
                                },
                                "evidence": {
                                    "type": "string",
                                    "description": "What tool result, source, or finding supports completion"
                                }
                            },
                            "required": ["step", "status"]
                        }
                    }
                },
                "required": ["plan"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let raw_items = args
            .get("plan")
            .or_else(|| args.get("items"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("missing 'plan' array"))?;
        let mut inputs = Vec::with_capacity(raw_items.len());
        for raw in raw_items {
            let step = required_str(raw, "step")?;
            let status = required_str(raw, "status")?;
            let id = optional_str(raw, "id");
            let evidence = optional_str(raw, "evidence");
            inputs.push(vault_plans::PlanItemInput {
                id,
                step,
                status,
                evidence,
            });
        }

        let rows = vault_plans::replace_current(
            &ctx.pool,
            &ctx.user_id,
            &ctx.session_id,
            ctx.task_id.as_deref(),
            &inputs,
        )
        .await?;
        let explanation = optional_str(&args, "explanation");
        let payload = serde_json::json!({
            "task_id": ctx.task_id,
            "explanation": explanation,
            "items": rows.iter().map(plan_item_json).collect::<Vec<_>>(),
        });
        let ts = Utc::now();
        let evt_seq = vault_events::insert(
            &ctx.pool,
            &ctx.user_id,
            &ctx.session_id,
            ctx.task_id.as_deref(),
            "plan_updated",
            &payload,
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
                    kind: "plan_updated".to_string(),
                    payload: payload.clone(),
                    ts,
                },
            )
            .await;

        let open = rows.iter().filter(|row| row.status != "completed").count();
        Ok(serde_json::json!({
            "status": "plan_updated",
            "open_items": open,
            "items": rows.iter().map(plan_item_json).collect::<Vec<_>>(),
            "guidance": if open == 0 {
                "active plan is complete; final answer may now synthesize the completed work"
            } else {
                "continue executing incomplete plan items; do not provide the final answer yet"
            }
        })
        .to_string())
    }
}

fn plan_item_json(row: &vault_plans::PlanItemRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.item_id,
        "seq": row.seq,
        "step": row.step,
        "status": row.status,
        "evidence": row.evidence,
    })
}

fn required_str(args: &serde_json::Value, key: &str) -> Result<String> {
    optional_str(args, key).ok_or_else(|| anyhow!("missing '{key}' argument"))
}

fn optional_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
