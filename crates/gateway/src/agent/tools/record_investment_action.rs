use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::events::EventEnvelope;
use crate::llm::ToolSpec;

use super::{ToolContext, ToolHandler};

const TOOL_NAME: &str = "record_investment_action";

pub struct RecordInvestmentActionTool;

impl RecordInvestmentActionTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for RecordInvestmentActionTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Record an investment action (decision draft) for the user to confirm or reject. \
                 Use this when the user explicitly asks to record, save, or lock in an investment \
                 decision. Do NOT use this for speculative discussion or analysis — only when the \
                 user is ready to commit. Writes a draft to the vault; the user must confirm via \
                 the UI before it becomes final. Returns the draft summary and a pending \
                 confirmation message."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ticker": {
                        "type": "string",
                        "description": "Ticker symbol, e.g. AAPL, 600519.SH, BTC"
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["long", "short", "close"],
                        "description": "Trade direction"
                    },
                    "size_pct": {
                        "type": "number",
                        "description": "Position size as % of portfolio (e.g. 3.5 for 3.5%)"
                    },
                    "stop_loss": {
                        "type": "number",
                        "description": "Stop-loss price (absolute)"
                    },
                    "target": {
                        "type": "number",
                        "description": "Price target (absolute)"
                    },
                    "horizon_days": {
                        "type": "integer",
                        "description": "Expected holding period in days"
                    },
                    "rationale": {
                        "type": "string",
                        "description": "Research rationale in markdown"
                    }
                },
                "required": ["ticker", "direction", "rationale"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let ticker = args
            .get("ticker")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'ticker' argument"))?
            .trim()
            .to_string();
        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'direction' argument"))?
            .trim()
            .to_string();
        let rationale = args
            .get("rationale")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'rationale' argument"))?
            .trim()
            .to_string();
        let size_pct = args.get("size_pct").and_then(|v| v.as_f64());
        let stop_loss = args.get("stop_loss").and_then(|v| v.as_f64());
        let target = args.get("target").and_then(|v| v.as_f64());
        let horizon_days = args.get("horizon_days").and_then(|v| v.as_i64());

        let deliverable_id = Uuid::now_v7().to_string();
        let now = Utc::now().to_rfc3339();

        let payload = serde_json::json!({
            "ticker": ticker,
            "direction": direction,
            "size_pct": size_pct,
            "stop_loss": stop_loss,
            "target": target,
            "horizon_days": horizon_days,
            "rationale": rationale,
            "session_id": ctx.session_id,
        });

        sqlx::query(
            r#"
            INSERT INTO deliverables
              (user_id, id, task_id, kind, payload_json, status, created_at, ready_at)
            VALUES (?, ?, ?, 'decision_draft', ?, 'pending_confirm', ?, ?)
            "#,
        )
        .bind(&ctx.user_id)
        .bind(&deliverable_id)
        .bind(ctx.task_id.as_deref())
        .bind(payload.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&ctx.pool)
        .await
        .context("inserting decision draft deliverable")?;

        let event_payload = serde_json::json!({
            "deliverable_id": deliverable_id,
            "ticker": ticker,
            "direction": direction,
            "size_pct": size_pct,
            "session_id": ctx.session_id,
        });
        let ts = Utc::now();
        ctx.event_bus
            .publish(
                &ctx.session_id,
                EventEnvelope {
                    seq: 0,
                    kind: "decision_draft_ready".to_string(),
                    payload: event_payload,
                    ts,
                },
            )
            .await;

        let direction_display = match direction.as_str() {
            "long" => "Long",
            "short" => "Short",
            "close" => "Close",
            other => other,
        };
        let size_row = size_pct
            .map(|v| format!("| 仓位 | {v:.1}% |\n"))
            .unwrap_or_default();
        let stop_row = stop_loss
            .map(|v| format!("| 止损 | {v} |\n"))
            .unwrap_or_default();
        let target_row = target
            .map(|v| format!("| 目标 | {v} |\n"))
            .unwrap_or_default();
        let horizon_row = horizon_days
            .map(|v| format!("| 期限 | {v} 天 |\n"))
            .unwrap_or_default();

        let summary = format!(
            "**决策草稿已提交，等待确认**\n\n\
             | 字段 | 值 |\n\
             |------|----|
             | 标的 | {ticker} |\n\
             | 方向 | {direction_display} |\n\
             {size_row}\
             {stop_row}\
             {target_row}\
             {horizon_row}\n\
             草稿 ID: `{deliverable_id}`\n\n\
             请在界面中确认或拒绝此决策。"
        );

        Ok(summary)
    }
}
