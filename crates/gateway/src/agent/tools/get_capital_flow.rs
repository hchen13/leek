use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{Datelike, Utc};
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_capital_flow";
const ENDPOINT: &str = "https://api.tushare.pro";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const DEFAULT_DAYS: i64 = 10;
const MAX_DAYS: i64 = 30;
const NORTHBOUND_DAILY_STOP_DATE: &str = "20240820";

pub struct GetCapitalFlowTool {
    http: Client,
}

impl GetCapitalFlowTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
    }

    async fn tushare_post(
        &self,
        payload: serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value> {
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.post(ENDPOINT).json(&payload).send() => r?,
        };
        if !resp.status().is_success() {
            bail!("tushare returned HTTP {}", resp.status().as_u16());
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = resp.json() => r?,
        };
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown tushare error");
            bail!("tushare error (code={code}): {msg}");
        }
        Ok(body)
    }

    async fn fetch_stock_flow(
        &self,
        token: &str,
        ts_code: &str,
        days: i64,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let payload = serde_json::json!({
            "api_name": "moneyflow",
            "token": token,
            "params": {"ts_code": ts_code, "limit": days},
            "fields": "trade_date,buy_elg_amount,sell_elg_amount,net_mf_amount,buy_lg_amount,sell_lg_amount,buy_md_amount,sell_md_amount,buy_sm_amount,sell_sm_amount"
        });
        let body = self.tushare_post(payload, cancel).await?;
        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("missing data in moneyflow response"))?;

        let fields: Vec<String> = data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let items: Vec<&Vec<serde_json::Value>> = data
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|row| row.as_array()).collect())
            .unwrap_or_default();

        if items.is_empty() {
            return Ok(format!(
                "[get_capital_flow: no moneyflow data for {ts_code}]"
            ));
        }

        let idx = |name: &str| -> Option<usize> { fields.iter().position(|f| f == name) };
        let i_date = idx("trade_date");
        let i_elg_buy = idx("buy_elg_amount");
        let i_elg_sell = idx("sell_elg_amount");
        let i_lg_buy = idx("buy_lg_amount");
        let i_lg_sell = idx("sell_lg_amount");
        let i_md_buy = idx("buy_md_amount");
        let i_md_sell = idx("sell_md_amount");
        let i_sm_buy = idx("buy_sm_amount");
        let i_sm_sell = idx("sell_sm_amount");
        let i_net = idx("net_mf_amount");

        let cell_f64 = |row: &Vec<serde_json::Value>, i: Option<usize>| -> f64 {
            i.and_then(|j| row.get(j))
                .and_then(|v| match v {
                    serde_json::Value::Number(n) => n.as_f64(),
                    serde_json::Value::String(s) => s.parse().ok(),
                    _ => None,
                })
                .unwrap_or(0.0)
        };

        let cell_str = |row: &Vec<serde_json::Value>, i: Option<usize>| -> String {
            i.and_then(|j| row.get(j))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => String::new(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };

        let mut rows: Vec<(String, f64, f64, f64, f64, f64)> = items
            .iter()
            .map(|row| {
                let date = cell_str(row, i_date);
                let elg_net = cell_f64(row, i_elg_buy) - cell_f64(row, i_elg_sell);
                let lg_net = cell_f64(row, i_lg_buy) - cell_f64(row, i_lg_sell);
                let md_net = cell_f64(row, i_md_buy) - cell_f64(row, i_md_sell);
                let sm_net = cell_f64(row, i_sm_buy) - cell_f64(row, i_sm_sell);
                let total_net = cell_f64(row, i_net);
                (date, elg_net, lg_net, md_net, sm_net, total_net)
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = format!("## {ts_code} · 资金流向（近{}日）\n\n", rows.len());
        out.push_str("| 日期       | 超大单净流入 | 大单净流入  | 中单净流入  | 小单净流入  | 净流入额    |\n");
        out.push_str(
            "|-----------|------------|-----------|-----------|-----------|------------|\n",
        );
        for (date, elg, lg, md, sm, net) in &rows {
            let date_fmt = if date.len() == 8 {
                format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
            } else {
                date.clone()
            };
            out.push_str(&format!(
                "| {:<10} | {:>12.1} | {:>11.1} | {:>11.1} | {:>11.1} | {:>11.1} |\n",
                date_fmt, elg, lg, md, sm, net
            ));
        }
        out.push_str(
            "\n_单位: 万元；分档列为买入额 - 卖出额，净流入额为 Tushare 原始 net_mf_amount。_\n",
        );
        Ok(out)
    }

    async fn fetch_northbound(
        &self,
        token: &str,
        days: i64,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let end = Utc::now().date_naive();
        let start = end - chrono::Duration::days(days * 2);

        let fmt_date =
            |d: chrono::NaiveDate| format!("{:04}{:02}{:02}", d.year(), d.month(), d.day());
        let end_date = fmt_date(end);
        if end_date.as_str() >= NORTHBOUND_DAILY_STOP_DATE {
            return Ok(northbound_daily_unavailable());
        }

        let payload = serde_json::json!({
            "api_name": "moneyflow_hsgt",
            "token": token,
            "params": {
                "start_date": fmt_date(start),
                "end_date": end_date
            },
            "fields": "trade_date,north_money,hgt,sgt,south_money"
        });
        let body = self.tushare_post(payload, cancel).await?;
        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("missing data in moneyflow_hsgt response"))?;

        let fields: Vec<String> = data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let items: Vec<&Vec<serde_json::Value>> = data
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|row| row.as_array()).collect())
            .unwrap_or_default();

        if items.is_empty() {
            return Ok("[get_capital_flow: no northbound data available]\n".to_string());
        }

        let idx = |name: &str| -> Option<usize> { fields.iter().position(|f| f == name) };
        let i_date = idx("trade_date");
        let i_north = idx("north_money");
        let i_hgt = idx("hgt");
        let i_sgt = idx("sgt");
        let i_south = idx("south_money");

        let cell_f64 = |row: &Vec<serde_json::Value>, i: Option<usize>| -> f64 {
            i.and_then(|j| row.get(j))
                .and_then(|v| match v {
                    serde_json::Value::Number(n) => n.as_f64(),
                    serde_json::Value::String(s) => s.parse().ok(),
                    _ => None,
                })
                .unwrap_or(0.0)
        };
        let cell_str = |row: &Vec<serde_json::Value>, i: Option<usize>| -> String {
            i.and_then(|j| row.get(j))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => String::new(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };

        let mut rows: Vec<(String, f64, f64, f64, f64)> = items
            .iter()
            .map(|row| {
                let date = cell_str(row, i_date);
                let north = cell_f64(row, i_north);
                let hgt = cell_f64(row, i_hgt);
                let sgt = cell_f64(row, i_sgt);
                let south = cell_f64(row, i_south);
                (date, north, hgt, sgt, south)
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        if rows.len() > days as usize {
            let drop = rows.len() - days as usize;
            rows.drain(..drop);
        }

        let mut out = format!("## 北向资金（近{}日）\n\n", rows.len());
        out.push_str(
            "| 日期       | 北向净流入(亿) | 沪股通(亿) | 深股通(亿) | 南向净流入(亿) |\n",
        );
        out.push_str("|-----------|-------------|----------|----------|-------------|\n");
        for (date, north, hgt, sgt, south) in &rows {
            let date_fmt = if date.len() == 8 {
                format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
            } else {
                date.clone()
            };
            out.push_str(&format!(
                "| {:<10} | {:>13.2} | {:>10.2} | {:>10.2} | {:>13.2} |\n",
                date_fmt,
                north / 10000.0,
                hgt / 10000.0,
                sgt / 10000.0,
                south / 10000.0,
            ));
        }
        out.push('\n');
        Ok(out)
    }
}

#[async_trait]
impl ToolHandler for GetCapitalFlowTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch A-share capital flow data: major/institutional money flow for individual stocks, \
                and historical north-bound capital (Hong Kong → Mainland via Stock Connect) market-wide flow. \
                Requires TUSHARE_TOKEN env var. \
                - For individual stock flow: provide ts_code. \
                - Daily north-bound net-flow disclosure stopped after 2024-08-20; for current dates this tool returns \
                  an explicit unavailable note instead of stale zero-like data."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "data_type": {
                        "type": "string",
                        "enum": ["stock_flow", "northbound", "both"],
                        "description": "Default: both"
                    },
                    "ts_code": {
                        "type": "string",
                        "description": "Required for stock_flow; omit for northbound-only. e.g. '600519.SH'"
                    },
                    "days": {
                        "type": "integer",
                        "description": "Number of trading days (default 10, max 30)"
                    }
                }
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let token = std::env::var("TUSHARE_TOKEN").map_err(|_| {
            anyhow!("TUSHARE_TOKEN env var not set — A-share data unavailable. Get a token at https://tushare.pro/register")
        })?;

        let data_type = args
            .get("data_type")
            .and_then(|v| v.as_str())
            .unwrap_or("both");
        let ts_code = args
            .get("ts_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase());
        let days = args
            .get("days")
            .and_then(|v| v.as_i64())
            .map(|n| n.clamp(1, MAX_DAYS))
            .unwrap_or(DEFAULT_DAYS);

        match data_type {
            "stock_flow" => {
                let code = ts_code
                    .ok_or_else(|| anyhow!("'ts_code' is required for stock_flow data_type"))?;
                self.fetch_stock_flow(&token, &code, days, &cancel).await
            }
            "northbound" => self.fetch_northbound(&token, days, &cancel).await,
            _ => {
                let mut out = String::new();
                if let Some(code) = ts_code {
                    out.push_str(&self.fetch_stock_flow(&token, &code, days, &cancel).await?);
                }
                out.push_str(&self.fetch_northbound(&token, days, &cancel).await?);
                if out.is_empty() {
                    bail!("'ts_code' is required for 'both' mode when stock_flow is included");
                }
                Ok(out)
            }
        }
    }
}

fn northbound_daily_unavailable() -> String {
    "## 北向资金（日度净流入已停更）\n\n\
     [get_capital_flow: northbound_daily_unavailable since 2024-08-20; \
     Tushare moneyflow_hsgt is historical-only for current analysis. \
     Use stock_flow for individual stocks, and use hk_hold quarterly holdings or hsgt_top10-style turnover data when those tools are wired.]\n"
        .to_string()
}
