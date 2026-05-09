use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::events::EventEnvelope;
use crate::llm::ToolSpec;
use crate::vault::events as vault_events;

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
                "Record a trade decision draft (long / short / close a position) for the user \
                 to confirm or reject in the UI. The semantic contract: the user is committing \
                 capital, not sharing an observation. Three signals must all be present — \
                 (1) a named ticker, (2) a clear direction, (3) language of commitment \
                 ('buy', 'build a position', 'close', 'enter', 'exit') rather than language of \
                 reflection ('interesting', 'note', 'agree', 'remember this', 'principle'). \
                 When the intent is to preserve an insight or guideline rather than execute a \
                 trade, use record_research_note instead.\n\n\
                 The structural fields (`risks`, `opposing_case`, `corpus_refs`, \
                 `mandate_check`, `invalidation_conditions`) are mandatory: a decision without \
                 a steelmanned bear case, named risks, an explicit invalidation, mandate \
                 alignment, and corpus grounding is by definition under-researched. If you \
                 cannot provide all of them honestly, do not record the action — go gather \
                 more facts first."
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
                        "description": "Research rationale in markdown (the bull case / why this trade)"
                    },
                    "risks": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "description": "Named, concrete risks. Must include at least one. Format each as 'risk: <one-line description>' — no platitudes like 'market conditions might change'."
                    },
                    "opposing_case": {
                        "type": "string",
                        "description": "The strongest steelmanned bear case (or bull case for shorts). Must engage with the *reasons* a smart investor would take the opposite side, not a strawman."
                    },
                    "corpus_refs": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Corpus document titles cited in the rationale (e.g. 'Margin of Safety', 'Long-term debt cycle'). Empty array allowed only if the corpus has no relevant page; explicitly state that condition in the rationale."
                    },
                    "mandate_check": {
                        "type": "object",
                        "properties": {
                            "fits_mandate": {
                                "type": "boolean",
                                "description": "Does this trade respect the user's mandate (risk tolerance, position caps, instrument limits)?"
                            },
                            "notes": {
                                "type": "string",
                                "description": "Specific mandate clauses that gate this trade. State them, don't summarize."
                            }
                        },
                        "required": ["fits_mandate", "notes"]
                    },
                    "invalidation_conditions": {
                        "type": "string",
                        "description": "What observation would falsify the thesis and force exit? E.g. 'EPS revisions turn negative for two consecutive quarters' or 'price closes below 50-week MA'. Must be observable and specific."
                    }
                },
                "required": [
                    "ticker", "direction", "rationale",
                    "risks", "opposing_case", "corpus_refs",
                    "mandate_check", "invalidation_conditions"
                ]
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

        let risks = parse_string_array(&args, "risks")?;
        if risks.is_empty() {
            bail!(
                "validation: 'risks' must contain at least one named, concrete risk. \
                 Do NOT retry with fabricated platitudes like 'market volatility'. \
                 If you don't have real risks for this trade, the right move is to \
                 NOT record the action and explain to the user that this draft cannot \
                 be submitted without a real risk list."
            );
        }
        let opposing_case = args
            .get("opposing_case")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'opposing_case' argument"))?
            .trim()
            .to_string();
        if opposing_case.is_empty() {
            bail!(
                "validation: 'opposing_case' must not be empty — record a steelmanned \
                 counter-thesis (engage with the reasons a smart investor would take \
                 the opposite side). Do NOT retry with a strawman or platitude; if you \
                 cannot supply a real counter-thesis, refuse to record and explain why."
            );
        }
        let corpus_refs = parse_string_array(&args, "corpus_refs")?;
        let mandate_check = args
            .get("mandate_check")
            .ok_or_else(|| anyhow!("missing 'mandate_check' argument"))?
            .clone();
        let mandate_obj = mandate_check
            .as_object()
            .ok_or_else(|| anyhow!("'mandate_check' must be an object"))?;
        if mandate_obj.get("fits_mandate").and_then(|v| v.as_bool()).is_none() {
            bail!("'mandate_check.fits_mandate' must be a boolean");
        }
        if mandate_obj
            .get("notes")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
        {
            bail!("'mandate_check.notes' must be a non-empty string");
        }
        let invalidation_conditions = args
            .get("invalidation_conditions")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'invalidation_conditions' argument"))?
            .trim()
            .to_string();
        if invalidation_conditions.is_empty() {
            bail!(
                "validation: 'invalidation_conditions' must not be empty — declare \
                 an observable falsifier (price level, EPS revision direction, \
                 regulatory event). Do NOT retry with 'if conditions change' or \
                 similar non-observable language."
            );
        }

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
            "risks": risks,
            "opposing_case": opposing_case,
            "corpus_refs": corpus_refs,
            "mandate_check": mandate_check,
            "invalidation_conditions": invalidation_conditions,
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

        // The SSE payload mirrors the deliverable so the UI card can show the
        // structural fields without a second fetch. `task_id` is included so
        // the chat panel can dedupe: the agent may iterate on a draft within
        // one task (critic-driven rewrite → record_investment_action again),
        // and we want to keep only the latest draft visible per task.
        let event_payload = serde_json::json!({
            "task_id": ctx.task_id,
            "deliverable_id": deliverable_id,
            "ticker": ticker,
            "direction": direction,
            "size_pct": size_pct,
            "stop_loss": stop_loss,
            "target": target,
            "horizon_days": horizon_days,
            "rationale": rationale,
            "risks": risks,
            "opposing_case": opposing_case,
            "corpus_refs": corpus_refs,
            "mandate_check": mandate_check,
            "invalidation_conditions": invalidation_conditions,
        });
        let ts = Utc::now();
        let evt_seq = vault_events::insert(
            &ctx.pool,
            &ctx.user_id,
            &ctx.session_id,
            ctx.task_id.as_deref(),
            "decision_draft_ready",
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
        let risks_row = format!("| 风险 | {} 项 |\n", risks.len());
        let mandate_fits = mandate_obj
            .get("fits_mandate")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mandate_row = format!(
            "| Mandate | {} |\n",
            if mandate_fits { "符合" } else { "需用户复核" }
        );

        let summary = format!(
            "**决策草稿已提交，等待确认**\n\n\
             | 字段 | 值 |\n\
             |------|----|
             | 标的 | {ticker} |\n\
             | 方向 | {direction_display} |\n\
             {size_row}\
             {stop_row}\
             {target_row}\
             {horizon_row}\
             {risks_row}\
             {mandate_row}\n\
             草稿 ID: `{deliverable_id}`\n\n\
             请在界面中确认或拒绝此决策。"
        );

        Ok(summary)
    }
}

fn parse_string_array(args: &serde_json::Value, key: &str) -> Result<Vec<String>> {
    let arr = args
        .get(key)
        .ok_or_else(|| anyhow!("missing '{key}' argument"))?
        .as_array()
        .ok_or_else(|| anyhow!("'{key}' must be an array of strings"))?;
    arr.iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| anyhow!("'{key}' entries must be strings"))
                .map(|s| s.trim().to_string())
        })
        .filter(|r| match r {
            Ok(s) => !s.is_empty(),
            Err(_) => true,
        })
        .collect()
}
