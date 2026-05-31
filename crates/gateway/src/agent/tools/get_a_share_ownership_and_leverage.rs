use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{data_provider_tokens, ToolContext, ToolHandler};

const TOOL_NAME: &str = "get_a_share_ownership_and_leverage";
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub struct GetAShareOwnershipAndLeverageTool {
    http: Client,
}

impl GetAShareOwnershipAndLeverageTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
    }

    async fn tushare_post(
        &self,
        token: &str,
        api_name: &str,
        params: serde_json::Value,
        fields: &str,
        cancel: &CancellationToken,
    ) -> Result<Table> {
        let client = TushareClient::with_client(token, self.http.clone())?;
        let response = client
            .query_cancelled(api_name, params, fields, cancel)
            .await?;
        Ok(Table::from_response(response))
    }
}

#[async_trait]
impl ToolHandler for GetAShareOwnershipAndLeverageTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch A-share ownership and leverage context: shareholder count trend, top holders, pledge risk, margin financing, and stock-connect activity. Use this for capital structure, crowding, leverage pressure, and holder-base analysis. Treat connect turnover as context, not as a direct thesis."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ts_code": {
                        "type": "string",
                        "description": "A-share security code, e.g. 600519.SH"
                    },
                    "sections": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["holders", "pledge", "margin", "connect"]
                        },
                        "description": "Default: holders, pledge, margin, connect"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Rows per section, default 5, max 20"
                    }
                },
                "required": ["ts_code"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let token = data_provider_tokens::tushare_token(ctx).await?;
        let ts_code = args
            .get("ts_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("ts_code is required"))?;
        let sections = sections(&args);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, 20))
            .unwrap_or(5);

        let mut out = format!("## {} · 筹码与杠杆上下文\n\n", ts_code);

        if sections.iter().any(|s| s == "holders") {
            let holder_count = self
                .tushare_post(
                    &token,
                    "stk_holdernumber",
                    serde_json::json!({"ts_code": ts_code, "limit": limit}),
                    "ts_code,ann_date,end_date,holder_num",
                    &cancel,
                )
                .await?;
            out.push_str("### 股东户数\n");
            render_rows(
                &mut out,
                &holder_count,
                limit,
                &["ann_date", "end_date", "holder_num"],
            );

            let top = self
                .tushare_post(
                    &token,
                    "top10_holders",
                    serde_json::json!({"ts_code": ts_code, "limit": limit * 10}),
                    "ts_code,end_date,ann_date,holder_name,hold_amount,hold_ratio",
                    &cancel,
                )
                .await?;
            out.push_str("\n### 前十大股东样本\n");
            render_rows(
                &mut out,
                &top,
                limit,
                &["end_date", "holder_name", "hold_amount", "hold_ratio"],
            );
        }

        if sections.iter().any(|s| s == "pledge") {
            let pledge = self
                .tushare_post(
                    &token,
                    "pledge_stat",
                    serde_json::json!({"ts_code": ts_code, "limit": limit}),
                    "ts_code,end_date,pledge_count,unrest_pledge,rest_pledge,total_share,pledge_ratio",
                    &cancel,
                )
                .await?;
            out.push_str("\n### 股权质押\n");
            render_rows(
                &mut out,
                &pledge,
                limit,
                &["end_date", "pledge_count", "total_share", "pledge_ratio"],
            );
        }

        if sections.iter().any(|s| s == "margin") {
            let margin = self
                .tushare_post(
                    &token,
                    "margin_detail",
                    serde_json::json!({"ts_code": ts_code, "limit": limit}),
                    "trade_date,ts_code,rzye,rzmre,rzche,rqye,rzrqye",
                    &cancel,
                )
                .await?;
            out.push_str("\n### 融资融券\n");
            render_rows(
                &mut out,
                &margin,
                limit,
                &["trade_date", "rzye", "rzmre", "rzche", "rqye", "rzrqye"],
            );
        }

        if sections.iter().any(|s| s == "connect") {
            out.push_str("\n### 沪深港通披露口径\n");
            out.push_str("- 沪深港通成交榜可作为活跃度/拥挤度线索，但不等同于持续持仓净流入。\n");
            let hsgt = self
                .tushare_post(
                    &token,
                    "hsgt_top10",
                    serde_json::json!({"ts_code": ts_code, "limit": limit}),
                    "trade_date,ts_code,name,close,change,rank,market_type,amount,net_amount,buy,sell",
                    &cancel,
                )
                .await;
            match hsgt {
                Ok(table) => render_rows(
                    &mut out,
                    &table,
                    limit,
                    &["trade_date", "rank", "amount", "net_amount", "buy", "sell"],
                ),
                Err(_) => {
                    out.push_str("- 沪深港通成交榜来源暂不可用；互联互通分析只保留为边界提示。\n")
                }
            }
        }

        out.push_str("\n_Caveat: 筹码、融资和互联互通数据是风险/拥挤度证据，不是独立买卖信号。_");
        Ok(out)
    }
}

struct Table {
    fields: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

impl Table {
    fn from_response(response: TushareResponse) -> Self {
        Self {
            fields: response.fields,
            rows: response.items,
        }
    }

    fn text(&self, row: &[serde_json::Value], field: &str) -> String {
        self.fields
            .iter()
            .position(|f| f == field)
            .and_then(|i| row.get(i))
            .map(value_text)
            .unwrap_or_default()
    }
}

fn render_rows(out: &mut String, table: &Table, limit: usize, fields: &[&str]) {
    if table.rows.is_empty() {
        out.push_str("- 当前来源没有返回记录；这是有效空结果或部分来源缺口，不代表事实不存在。\n");
        return;
    }
    for row in table.rows.iter().take(limit) {
        let summary = fields
            .iter()
            .filter_map(|field| {
                let value = table.text(row, field);
                if value.is_empty() {
                    None
                } else {
                    Some(format!("{}={value}", ownership_field_label(field)))
                }
            })
            .collect::<Vec<_>>()
            .join(" · ");
        out.push_str("- ");
        out.push_str(&summary);
        out.push('\n');
    }
}

fn ownership_field_label(field: &str) -> &str {
    match field {
        "ann_date" => "公告日",
        "end_date" => "报告期",
        "holder_num" => "股东户数",
        "holder_name" => "股东",
        "hold_amount" => "持股数",
        "hold_ratio" => "持股比例",
        "pledge_count" => "质押笔数",
        "total_share" => "质押总股数",
        "pledge_ratio" => "质押比例",
        "trade_date" => "交易日",
        "rzye" => "融资余额",
        "rzmre" => "融资买入",
        "rzche" => "融资偿还",
        "rqye" => "融券余额",
        "rzrqye" => "两融余额",
        "rank" => "排名",
        "amount" => "成交额",
        "net_amount" => "净买入",
        "buy" => "买入",
        "sell" => "卖出",
        _ => field,
    }
}

fn sections(args: &serde_json::Value) -> Vec<String> {
    args.get("sections")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            vec![
                "holders".to_string(),
                "pledge".to_string(),
                "margin".to_string(),
                "connect".to_string(),
            ]
        })
}

fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sections_cover_all_domains() {
        let s = sections(&serde_json::json!({}));
        assert!(s.contains(&"holders".to_string()));
        assert!(s.contains(&"connect".to_string()));
    }

    #[test]
    fn render_rows_distinguishes_empty() {
        let mut out = String::new();
        let table = Table {
            fields: vec!["x".into()],
            rows: Vec::new(),
        };
        render_rows(&mut out, &table, 5, &["x"]);
        assert!(out.contains("有效空结果"));
    }
}
