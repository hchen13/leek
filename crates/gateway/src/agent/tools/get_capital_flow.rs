use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::{data_provider_tokens, ToolContext, ToolHandler};

const TOOL_NAME: &str = "get_capital_flow";
const ENDPOINT: &str = "https://api.tushare.pro";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const DEFAULT_DAYS: i64 = 10;
const MAX_DAYS: i64 = 30;
const DEFAULT_BLOCK_LIMIT: usize = 5;
const MAX_BLOCK_LIMIT: usize = 10;
const EASTMONEY_UT: &str = "b2884a393a59ad64002292a3e90d46a5";
const EASTMONEY_FLOW_FIELDS: &str =
    "f12,f14,f2,f3,f6,f62,f184,f66,f69,f72,f75,f78,f81,f84,f87,f124";
const EASTMONEY_BLOCK_FLOW_FIELDS: &str = "f1,f2,f3,f4,f12,f13,f14,f128,f140,f141,f62";
const EASTMONEY_FLOW_URLS: [(&str, &str); 2] = [
    (
        "https://push2.eastmoney.com/api/qt/ulist.np/get",
        "eastmoney_push2",
    ),
    (
        "https://push2delay.eastmoney.com/api/qt/ulist.np/get",
        "eastmoney_push2delay",
    ),
];
const EASTMONEY_BLOCK_FLOW_URL: &str = "https://emdatah5.eastmoney.com/dc/ZJLX/getZDYLBData";

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
        let name = super::ashare_security_name(&self.http, token, ts_code, cancel).await;
        let label = name
            .map(|n| format!("{n}（{ts_code}）"))
            .unwrap_or_else(|| ts_code.to_string());
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
                "## {label} · 日频资金流向\n\n\
                 - 当前日频资金流来源没有返回记录。这是所选来源和参数下的有效空结果，不代表资金流为零；使用前需核验代码与来源覆盖。\n"
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

        let mut out = format!("## {label} · 日频资金流向（近{}日）\n\n", rows.len());
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

    async fn fetch_eastmoney_realtime_flow(
        &self,
        ts_code: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let secid = eastmoney_secid(ts_code)?;
        let mut last_error = None;

        for (url, source) in EASTMONEY_FLOW_URLS {
            match self
                .fetch_eastmoney_realtime_flow_from(url, source, &secid, ts_code, cancel)
                .await
            {
                Ok(out) => return Ok(out),
                Err(err) if is_abort_error(&err, cancel) => return Err(err),
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Eastmoney realtime flow source unavailable")))
    }

    async fn fetch_eastmoney_realtime_flow_from(
        &self,
        url: &str,
        source: &str,
        secid: &str,
        ts_code: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let request = self
            .http
            .get(url)
            .query(&[
                ("fltt", "2"),
                ("secids", secid),
                ("fields", EASTMONEY_FLOW_FIELDS),
                ("ut", EASTMONEY_UT),
            ])
            .build()?;
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney flow request"),
            result = self.http.execute(request) => result?,
        };
        let status = response.status();
        if !status.is_success() {
            bail!("Eastmoney flow returned HTTP {status}");
        }
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney flow response body"),
            result = response.text() => result?,
        };
        let body: EastmoneyFlowResponse = serde_json::from_str(&text).map_err(|err| {
            anyhow!(
                "Eastmoney flow returned invalid JSON: {err}; sample: {}",
                compact_text(&text, 160)
            )
        })?;
        if body.rc != Some(0) {
            bail!(
                "Eastmoney flow returned rc={:?}: {}",
                body.rc,
                body.message_text().unwrap_or("unknown error")
            );
        }

        let rows = body.data.and_then(|data| data.diff).unwrap_or_default();
        let source_label = eastmoney_flow_source_label(source);
        let mut out = format!("## {ts_code} · 盘中资金流向 · {source_label}\n\n");
        out.push_str("| 代码 | 名称 | 更新时间 | 最新价 | 涨跌幅% | 成交额(元) | 主力净流入(元) | 主力净占比% | 超大单净流入(元) | 超大单净占比% | 大单净流入(元) | 大单净占比% | 中单净流入(元) | 中单净占比% | 小单净流入(元) | 小单净占比% |\n");
        out.push_str(
            "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
        );

        if rows.is_empty() {
            out.push_str("| - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - |\n");
            out.push_str(
                "\n_当前盘中资金流来源没有返回记录；这是有效空结果，不代表资金流为零。_\n",
            );
        } else {
            for row in rows {
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    value_text_opt(row.f12.as_ref()),
                    value_text_opt(row.f14.as_ref()),
                    eastmoney_epoch_text(row.f124.as_ref()),
                    value_text_opt(row.f2.as_ref()),
                    value_text_opt(row.f3.as_ref()),
                    value_text_opt(row.f6.as_ref()),
                    value_text_opt(row.f62.as_ref()),
                    value_text_opt(row.f184.as_ref()),
                    value_text_opt(row.f66.as_ref()),
                    value_text_opt(row.f69.as_ref()),
                    value_text_opt(row.f72.as_ref()),
                    value_text_opt(row.f75.as_ref()),
                    value_text_opt(row.f78.as_ref()),
                    value_text_opt(row.f81.as_ref()),
                    value_text_opt(row.f84.as_ref()),
                    value_text_opt(row.f87.as_ref())
                ));
            }
        }
        out.push_str("\n_来源: 东方财富公开盘中资金流；单位为元/百分比。它补充当前交易时段观察，不替代日频历史口径，也不要与万元字段直接相加。_\n");
        Ok(out)
    }

    async fn fetch_market_block_flow(
        &self,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let market = self
            .fetch_eastmoney_index_flow("沪深两市", "1.000001,0.399001", cancel)
            .await?;
        let industry_inflow = self
            .fetch_eastmoney_block_flow("行业主力净流入", "m:90+t:2", 1, limit, cancel)
            .await?;
        let industry_outflow = self
            .fetch_eastmoney_block_flow("行业主力净流出", "m:90+t:2", 0, limit, cancel)
            .await?;
        let concept_inflow = self
            .fetch_eastmoney_block_flow("概念主力净流入", "m:90+t:3", 1, limit, cancel)
            .await?;
        let concept_outflow = self
            .fetch_eastmoney_block_flow("概念主力净流出", "m:90+t:3", 0, limit, cancel)
            .await?;

        let mut out = "## 市场/板块资金流向 · 东方财富\n\n".to_string();
        out.push_str(&market);
        out.push('\n');
        out.push_str(&industry_inflow);
        out.push('\n');
        out.push_str(&industry_outflow);
        out.push('\n');
        out.push_str(&concept_inflow);
        out.push('\n');
        out.push_str(&concept_outflow);
        out.push_str("\n_来源: 东方财富公开资金流页面。市场指数资金流为沪深两市指数快照合并；板块榜单为公开页面排行。单位为元/百分比，适合观察当日资金主线，不等同于可审计的历史日频结论。_\n");
        Ok(out)
    }

    async fn fetch_eastmoney_index_flow(
        &self,
        title: &str,
        secids: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let rows = self.fetch_eastmoney_flow_rows(secids, cancel).await?;
        if rows.is_empty() {
            return Ok(format!(
                "### {title}\n- 当前指数资金流来源没有返回记录；这是有效空结果，不代表资金流为零。\n"
            ));
        }

        let mut total = FlowAccumulator::default();
        let mut latest_update = String::new();
        for row in &rows {
            total.add(row);
            let update = eastmoney_epoch_text(row.f124.as_ref());
            if update > latest_update {
                latest_update = update;
            }
        }

        let mut out = format!("### {title}\n");
        out.push_str("| 更新时间 | 成交额(元) | 主力净流入(元) | 主力净占比% | 超大单净流入(元) | 大单净流入(元) | 中单净流入(元) | 小单净流入(元) |\n");
        out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            latest_update,
            fmt_num(total.amount),
            fmt_num(total.main_net),
            fmt_pct_from_amount(total.main_net, total.amount),
            fmt_num(total.elg_net),
            fmt_num(total.lg_net),
            fmt_num(total.md_net),
            fmt_num(total.sm_net)
        ));
        Ok(out)
    }

    async fn fetch_eastmoney_block_flow(
        &self,
        title: &str,
        fs: &str,
        po: i32,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let request = self
            .http
            .get(EASTMONEY_BLOCK_FLOW_URL)
            .query(&[
                ("fields", EASTMONEY_BLOCK_FLOW_FIELDS.to_string()),
                ("pn", "1".to_string()),
                ("pz", limit.to_string()),
                ("fid", "f62".to_string()),
                ("po", po.to_string()),
                ("fs", fs.to_string()),
                ("ut", EASTMONEY_UT.to_string()),
            ])
            .build()?;
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney block-flow request"),
            result = self.http.execute(request) => result?,
        };
        let status = response.status();
        if !status.is_success() {
            bail!("Eastmoney block flow returned HTTP {status}");
        }
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney block-flow response body"),
            result = response.text() => result?,
        };
        let body: EastmoneyBlockFlowResponse = serde_json::from_str(&text).map_err(|err| {
            anyhow!(
                "Eastmoney block flow returned invalid JSON: {err}; sample: {}",
                compact_text(&text, 160)
            )
        })?;
        if body.rc != Some(0) {
            bail!(
                "Eastmoney block flow returned rc={:?}: {}",
                body.rc,
                body.message_text().unwrap_or("unknown error")
            );
        }

        let rows = body.data.and_then(|data| data.diff).unwrap_or_default();
        let mut out = format!("### {title}\n");
        out.push_str("| 板块代码 | 板块 | 最新点位 | 涨跌幅% | 主力净流入(元) | 领涨股 |\n");
        out.push_str("|---|---|---:|---:|---:|---|\n");
        if rows.is_empty() {
            out.push_str("| - | 无返回记录 | - | - | - | - |\n");
            out.push_str(
                "\n_当前板块资金流来源没有返回记录；这是有效空结果，不代表板块资金面不存在。_\n",
            );
        } else {
            for row in rows {
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {}{} |\n",
                    value_text_opt(row.f12.as_ref()),
                    value_text_opt(row.f14.as_ref()),
                    value_text_opt(row.f2.as_ref()),
                    value_text_opt(row.f3.as_ref()),
                    value_text_opt(row.f62.as_ref()),
                    value_text_opt(row.f128.as_ref()),
                    suffix_code(row.f140.as_ref())
                ));
            }
        }
        Ok(out)
    }

    async fn fetch_eastmoney_flow_rows(
        &self,
        secids: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<EastmoneyFlowRow>> {
        let mut last_error = None;

        for (url, _source) in EASTMONEY_FLOW_URLS {
            match self
                .fetch_eastmoney_flow_rows_from(url, secids, cancel)
                .await
            {
                Ok(rows) => return Ok(rows),
                Err(err) if is_abort_error(&err, cancel) => return Err(err),
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Eastmoney flow source unavailable")))
    }

    async fn fetch_eastmoney_flow_rows_from(
        &self,
        url: &str,
        secids: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<EastmoneyFlowRow>> {
        let request = self
            .http
            .get(url)
            .query(&[
                ("fltt", "2"),
                ("secids", secids),
                ("fields", EASTMONEY_FLOW_FIELDS),
                ("ut", EASTMONEY_UT),
            ])
            .build()?;
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney flow request"),
            result = self.http.execute(request) => result?,
        };
        let status = response.status();
        if !status.is_success() {
            bail!("Eastmoney flow returned HTTP {status}");
        }
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney flow response body"),
            result = response.text() => result?,
        };
        let body: EastmoneyFlowResponse = serde_json::from_str(&text).map_err(|err| {
            anyhow!(
                "Eastmoney flow returned invalid JSON: {err}; sample: {}",
                compact_text(&text, 160)
            )
        })?;
        if body.rc != Some(0) {
            bail!(
                "Eastmoney flow returned rc={:?}: {}",
                body.rc,
                body.message_text().unwrap_or("unknown error")
            );
        }
        Ok(body.data.and_then(|data| data.diff).unwrap_or_default())
    }

    /// 大盘资金流 — 东方财富 `moneyflow_mkt_dc`, last `days` trading days. Net
    /// amounts arrive in 元 and are rendered in 亿. Queries a calendar window
    /// wide enough to cover `days` trading days across weekends/holidays.
    async fn fetch_market_flow(
        &self,
        token: &str,
        days: i64,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let (start_date, end_date) = recent_window(days * 2 + 12);
        let payload = serde_json::json!({
            "api_name": "moneyflow_mkt_dc",
            "token": token,
            "params": {"start_date": start_date, "end_date": end_date},
            "fields": "trade_date,close_sh,pct_change_sh,close_sz,pct_change_sz,net_amount,buy_elg_amount,buy_lg_amount,buy_md_amount,buy_sm_amount"
        });
        let body = self.tushare_post(payload, cancel).await?;
        let (fields, items) = tushare_data(&body)?;
        Ok(render_market_flow(&fields, &items, days.max(1) as usize))
    }

    /// 行业/概念板块资金流 — 东方财富 `moneyflow_ind_dc`. Resolves the latest
    /// trading day from 大盘 flow (cheap, bounded), then pulls that day's board
    /// ranking. `content_type` is the Chinese vendor enum (行业/概念/地域); the
    /// English values in the published doc return zero rows.
    async fn fetch_board_flow(
        &self,
        token: &str,
        content_type: &str,
        label: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let Some(date) = self.latest_trade_date(token, cancel).await? else {
            return Ok(format!(
                "## {label}资金流\n\n- 板块资金流来源近期没有返回交易日。这是来源覆盖缺口，不代表无资金流。\n"
            ));
        };
        let payload = serde_json::json!({
            "api_name": "moneyflow_ind_dc",
            "token": token,
            "params": {"trade_date": date, "content_type": content_type},
            "fields": "trade_date,name,pct_change,net_amount,buy_sm_amount_stock"
        });
        let body = self.tushare_post(payload, cancel).await?;
        let (fields, items) = tushare_data(&body)?;
        Ok(render_board_flow(&fields, &items, label, &date, limit))
    }

    /// Latest trading day with 东财 flow data, via a small `moneyflow_mkt_dc`
    /// window (≤ ~20 rows) so it survives long holidays without risking the
    /// per-request row cap that a wide board query would hit.
    async fn latest_trade_date(
        &self,
        token: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        let (start_date, end_date) = recent_window(20);
        let payload = serde_json::json!({
            "api_name": "moneyflow_mkt_dc",
            "token": token,
            "params": {"start_date": start_date, "end_date": end_date},
            "fields": "trade_date"
        });
        let body = self.tushare_post(payload, cancel).await?;
        let (fields, items) = tushare_data(&body)?;
        let i_date = fields.iter().position(|f| f == "trade_date");
        Ok(items
            .iter()
            .filter_map(|row| i_date.and_then(|j| row.get(j)).map(value_text))
            .filter(|s| !s.is_empty())
            .max())
    }

    /// Northbound now has exactly one surviving caliber: the daily 沪深股通
    /// top-10 most-traded stocks with turnover. The 2024-08-18 Stock Connect
    /// change stopped buy/sell/net disclosure (and silently turned the old
    /// `moneyflow_hsgt` net-flow fields into always-positive turnover), so this
    /// returns activity only — explicitly labeled as not net inflow.
    async fn fetch_northbound(&self, token: &str, cancel: &CancellationToken) -> Result<String> {
        let (start_date, end_date) = recent_window(14);
        let payload = serde_json::json!({
            "api_name": "hsgt_top10",
            "token": token,
            "params": {"start_date": start_date, "end_date": end_date},
            "fields": "trade_date,ts_code,name,market_type,amount"
        });
        let body = self.tushare_post(payload, cancel).await?;
        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("missing data in hsgt_top10 response"))?;
        let fields: Vec<String> = data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let items: Vec<Vec<serde_json::Value>> = data
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|row| row.as_array().cloned())
                    .collect()
            })
            .unwrap_or_default();
        Ok(render_northbound_top10(&fields, &items))
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
            description: "Fetch A-share capital-flow evidence at task level, not raw endpoint dumps. \
                Stock level: `stock_flow` (official daily historical moneyflow for one stock), `realtime_stock_flow` (public intraday/main-force snapshot). \
                Market & board level (no ts_code, latest trading day, structured — use these instead of scraping data-vendor URLs): `market_flow` (大盘 daily main-force net flow), `industry_flow` (行业板块 net-flow ranking), `concept_flow` (概念板块 net-flow ranking). \
                `market_block_flow` is a public intraday market/sector snapshot. \
                `northbound` returns the 沪深股通 top-10 most-traded A-shares with daily turnover, split 沪股通/深股通. \
                `both` returns stock-level history + snapshot when ts_code is provided, plus the market/sector snapshot and the northbound active-stock list. \
                - Northbound is turnover/activity only: after the 2024-08-18 Stock Connect change, buy/sell/net-flow stopped being disclosed, so do NOT read it as net inflow. \
                Preserve source and units: stock moneyflow is 万元; 大盘/板块 net flow is rendered in 亿元; intraday snapshot rows are 元. Main-force (主力) = 超大单 + 大单."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "data_type": {
                        "type": "string",
                        "enum": ["stock_flow", "realtime_stock_flow", "market_flow", "industry_flow", "concept_flow", "market_block_flow", "northbound", "both"],
                        "description": "Default: both. market_flow/industry_flow/concept_flow give structured 大盘/行业/概念 daily net flow and need no ts_code. With ts_code, both returns stock_flow + realtime_stock_flow + market_block_flow + northbound; without ts_code, both returns market_block_flow + northbound."
                    },
                    "ts_code": {
                        "type": "string",
                        "description": "Required for stock_flow and realtime_stock_flow; omit for market_flow/industry_flow/concept_flow/northbound. e.g. '600519.SH'"
                    },
                    "days": {
                        "type": "integer",
                        "description": "Trading days for stock_flow / market_flow (default 10, max 30)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Rows per ranking section — market/sector snapshot, and each of the industry/concept inflow & outflow lists. Default 5, max 10."
                    }
                }
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
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
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_BLOCK_LIMIT))
            .unwrap_or(DEFAULT_BLOCK_LIMIT);

        match data_type {
            "stock_flow" => {
                let code = ts_code
                    .ok_or_else(|| anyhow!("'ts_code' is required for stock_flow data_type"))?;
                let token = data_provider_tokens::tushare_token(ctx).await?;
                self.fetch_stock_flow(&token, &code, days, &cancel).await
            }
            "realtime_stock_flow" => {
                let code = ts_code.ok_or_else(|| {
                    anyhow!("'ts_code' is required for realtime_stock_flow data_type")
                })?;
                self.fetch_eastmoney_realtime_flow(&code, &cancel).await
            }
            "market_block_flow" => self.fetch_market_block_flow(limit, &cancel).await,
            "market_flow" => {
                let token = data_provider_tokens::tushare_token(ctx).await?;
                self.fetch_market_flow(&token, days, &cancel).await
            }
            "industry_flow" => {
                let token = data_provider_tokens::tushare_token(ctx).await?;
                self.fetch_board_flow(&token, "行业", "行业板块", limit, &cancel)
                    .await
            }
            "concept_flow" => {
                let token = data_provider_tokens::tushare_token(ctx).await?;
                self.fetch_board_flow(&token, "概念", "概念板块", limit, &cancel)
                    .await
            }
            "northbound" => {
                let token = data_provider_tokens::tushare_token(ctx).await?;
                self.fetch_northbound(&token, &cancel).await
            }
            "both" => {
                let mut out = String::new();
                if let Some(code) = ts_code {
                    let stock_flow = match data_provider_tokens::tushare_token(ctx).await {
                        Ok(token) => self.fetch_stock_flow(&token, &code, days, &cancel).await,
                        Err(err) => Err(err),
                    };
                    push_section_result(
                        &mut out,
                        "个股日频资金流",
                        "tushare_moneyflow",
                        stock_flow,
                        &cancel,
                    )?;
                    let realtime_flow = self.fetch_eastmoney_realtime_flow(&code, &cancel).await;
                    push_section_result(
                        &mut out,
                        "个股盘中资金流",
                        "eastmoney_realtime_flow",
                        realtime_flow,
                        &cancel,
                    )?;
                }
                let market_block_flow = self.fetch_market_block_flow(limit, &cancel).await;
                push_section_result(
                    &mut out,
                    "市场/板块资金流",
                    "eastmoney_market_block_flow",
                    market_block_flow,
                    &cancel,
                )?;
                let northbound = match data_provider_tokens::tushare_token(ctx).await {
                    Ok(token) => self.fetch_northbound(&token, &cancel).await,
                    Err(err) => Err(err),
                };
                push_section_result(
                    &mut out,
                    "北向成交活跃股",
                    "tushare_hsgt_top10",
                    northbound,
                    &cancel,
                )?;
                Ok(out)
            }
            other => bail!("unsupported data_type: {other}"),
        }
    }
}

#[derive(Deserialize)]
struct EastmoneyFlowResponse {
    rc: Option<i64>,
    data: Option<EastmoneyFlowData>,
    msg: Option<String>,
    message: Option<String>,
}

impl EastmoneyFlowResponse {
    fn message_text(&self) -> Option<&str> {
        clean_opt(self.message.as_deref()).or_else(|| clean_opt(self.msg.as_deref()))
    }
}

#[derive(Deserialize)]
struct EastmoneyFlowData {
    diff: Option<Vec<EastmoneyFlowRow>>,
}

#[derive(Deserialize)]
struct EastmoneyFlowRow {
    f2: Option<serde_json::Value>,
    f3: Option<serde_json::Value>,
    f6: Option<serde_json::Value>,
    f12: Option<serde_json::Value>,
    f14: Option<serde_json::Value>,
    f62: Option<serde_json::Value>,
    f66: Option<serde_json::Value>,
    f69: Option<serde_json::Value>,
    f72: Option<serde_json::Value>,
    f75: Option<serde_json::Value>,
    f78: Option<serde_json::Value>,
    f81: Option<serde_json::Value>,
    f84: Option<serde_json::Value>,
    f87: Option<serde_json::Value>,
    f124: Option<serde_json::Value>,
    f184: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct EastmoneyBlockFlowResponse {
    rc: Option<i64>,
    data: Option<EastmoneyBlockFlowData>,
    msg: Option<String>,
    message: Option<String>,
}

impl EastmoneyBlockFlowResponse {
    fn message_text(&self) -> Option<&str> {
        clean_opt(self.message.as_deref()).or_else(|| clean_opt(self.msg.as_deref()))
    }
}

#[derive(Deserialize)]
struct EastmoneyBlockFlowData {
    diff: Option<Vec<EastmoneyBlockFlowRow>>,
}

#[derive(Deserialize)]
struct EastmoneyBlockFlowRow {
    f2: Option<serde_json::Value>,
    f3: Option<serde_json::Value>,
    f12: Option<serde_json::Value>,
    f14: Option<serde_json::Value>,
    f62: Option<serde_json::Value>,
    f128: Option<serde_json::Value>,
    f140: Option<serde_json::Value>,
}

#[derive(Default)]
struct FlowAccumulator {
    amount: f64,
    main_net: f64,
    elg_net: f64,
    lg_net: f64,
    md_net: f64,
    sm_net: f64,
}

impl FlowAccumulator {
    fn add(&mut self, row: &EastmoneyFlowRow) {
        self.amount += value_f64(row.f6.as_ref());
        self.main_net += value_f64(row.f62.as_ref());
        self.elg_net += value_f64(row.f66.as_ref());
        self.lg_net += value_f64(row.f72.as_ref());
        self.md_net += value_f64(row.f78.as_ref());
        self.sm_net += value_f64(row.f84.as_ref());
    }
}

fn push_section_result(
    out: &mut String,
    title: &'static str,
    source: &'static str,
    result: Result<String>,
    cancel: &CancellationToken,
) -> Result<()> {
    match result {
        Ok(section) => {
            out.push_str(&section);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        Err(err) if is_abort_error(&err, cancel) => return Err(err),
        Err(err) => out.push_str(&unavailable_section(title, source, &err)),
    }
    Ok(())
}

fn unavailable_section(title: &str, source: &str, err: &anyhow::Error) -> String {
    format!(
        "## {title} · {} · 来源不可用\n\n- 来源不可用：{}。这代表覆盖/访问缺口，不是有效空结果。\n\n",
        capital_flow_source_label(source),
        compact_text(&err.to_string(), 180)
    )
}

fn eastmoney_flow_source_label(source: &str) -> &'static str {
    match source {
        "eastmoney_push2" => "公开实时快照",
        "eastmoney_push2delay" => "公开延迟快照",
        _ => "公开盘中快照",
    }
}

fn capital_flow_source_label(source: &str) -> &'static str {
    match source {
        "tushare_moneyflow" => "日频资金流",
        "eastmoney_realtime_flow" => "盘中资金流",
        "eastmoney_market_block_flow" => "市场/板块资金流",
        "tushare_hsgt_top10" => "北向成交活跃股",
        _ => "资金流来源",
    }
}

fn eastmoney_secid(ts_code: &str) -> Result<String> {
    let code = ts_code.trim().to_uppercase();
    if let Some(symbol) = code.strip_suffix(".SH") {
        return Ok(format!("1.{symbol}"));
    }
    if let Some(symbol) = code
        .strip_suffix(".SZ")
        .or_else(|| code.strip_suffix(".BJ"))
    {
        return Ok(format!("0.{symbol}"));
    }
    bail!("Eastmoney realtime flow expects A-share ts_code with .SH, .SZ, or .BJ suffix");
}

fn eastmoney_epoch_text(v: Option<&serde_json::Value>) -> String {
    let Some(ts) = v.and_then(value_i64) else {
        return value_text_opt(v);
    };
    let Some(dt) = DateTime::from_timestamp(ts, 0) else {
        return ts.to_string();
    };
    let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8 offset is valid");
    dt.with_timezone(&offset)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn value_i64(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn value_f64(v: Option<&serde_json::Value>) -> f64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn value_text_opt(v: Option<&serde_json::Value>) -> String {
    v.map(value_text).unwrap_or_default()
}

fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn suffix_code(v: Option<&serde_json::Value>) -> String {
    let code = value_text_opt(v);
    if code.is_empty() {
        String::new()
    } else {
        format!("({code})")
    }
}

fn fmt_num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.2}")
    } else {
        String::new()
    }
}

fn fmt_pct_from_amount(numerator: f64, denominator: f64) -> String {
    if denominator.abs() <= f64::EPSILON {
        return String::new();
    }
    format!("{:.2}", numerator / denominator * 100.0)
}

fn clean_opt(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty() && *s != "-")
}

fn is_abort_error(err: &anyhow::Error, cancel: &CancellationToken) -> bool {
    cancel.is_cancelled() || err.to_string().to_lowercase().contains("aborted")
}

fn compact_text(s: &str, max_chars: usize) -> String {
    let compact = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let prefix = compact.chars().take(max_chars).collect::<String>();
    format!("{prefix}...")
}

/// Render the only surviving northbound caliber — 沪深股通十大成交活跃股 + 当日成交额
/// — from a raw `hsgt_top10` response. Picks the latest trade_date present (the
/// window may span several days), splits 沪股通 (market_type 1) from 深股通
/// (market_type 3), and labels the caliber honestly: turnover/activity only,
/// because buy/sell/net stopped at 2024-08-18.
fn render_northbound_top10(fields: &[String], items: &[Vec<serde_json::Value>]) -> String {
    let idx = |name: &str| -> Option<usize> { fields.iter().position(|f| f == name) };
    let (i_date, i_code, i_name, i_mkt, i_amount) = (
        idx("trade_date"),
        idx("ts_code"),
        idx("name"),
        idx("market_type"),
        idx("amount"),
    );
    fn cell(row: &[serde_json::Value], i: Option<usize>) -> Option<&serde_json::Value> {
        i.and_then(|j| row.get(j))
    }

    let latest = items
        .iter()
        .filter_map(|row| cell(row, i_date).map(value_text))
        .filter(|s| !s.is_empty())
        .max();
    let Some(latest) = latest else {
        return "## 北向成交活跃股（沪深股通）\n\n\
                - 互联互通成交活跃股来源近 14 日没有返回记录。这是有效空结果或来源覆盖缺口，不代表北向无成交。\n"
            .to_string();
    };

    // (amount, name, code)
    let mut hu: Vec<(f64, String, String)> = Vec::new();
    let mut shen: Vec<(f64, String, String)> = Vec::new();
    for row in items {
        if cell(row, i_date).map(value_text).as_deref() != Some(latest.as_str()) {
            continue;
        }
        let entry = (
            value_f64(cell(row, i_amount)),
            value_text_opt(cell(row, i_name)),
            value_text_opt(cell(row, i_code)),
        );
        match cell(row, i_mkt).and_then(value_i64) {
            Some(1) => hu.push(entry),
            Some(3) => shen.push(entry),
            _ => {}
        }
    }

    let date_fmt = if latest.len() == 8 {
        format!("{}-{}-{}", &latest[..4], &latest[4..6], &latest[6..])
    } else {
        latest.clone()
    };
    let mut out = format!("## 北向成交活跃股（沪深股通 · {date_fmt}）\n\n");
    out.push_str(
        "口径说明：2024-08-18 起沪深港通不再披露北向买入/卖出/净额，仅保留十大成交活跃股的当日成交额。\
         以下是活跃度/拥挤度信号，不是净流入；成交额含买卖双边，不要与个股资金流的净额相加。\n\n",
    );
    render_northbound_market(&mut out, "沪股通", &mut hu);
    render_northbound_market(&mut out, "深股通", &mut shen);
    out.push_str(
        "\n_来源: 沪深证券交易所互联互通披露（沪深股通十大成交活跃股）。仅成交额口径，买卖净额自 2024-08-18 起停止披露。_\n",
    );
    out
}

fn render_northbound_market(out: &mut String, label: &str, rows: &mut [(f64, String, String)]) {
    out.push_str(&format!("### {label}十大成交活跃股（按成交额）\n\n"));
    if rows.is_empty() {
        out.push_str("- 当日无该市场成交活跃股记录。\n\n");
        return;
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out.push_str("| 名称 | 代码 | 成交额(亿) |\n|------|------|-----------|\n");
    let mut total = 0.0;
    for (amount, name, code) in rows.iter() {
        total += *amount;
        out.push_str(&format!("| {} | {} | {:.2} |\n", name, code, amount / 1e8));
    }
    out.push_str(&format!(
        "\n{label}十大合计成交额 {:.2} 亿。\n\n",
        total / 1e8
    ));
}

/// (start_date, end_date) as YYYYMMDD for a Beijing-time window ending today.
fn recent_window(days_back: i64) -> (String, String) {
    let tz = FixedOffset::east_opt(8 * 3600).expect("UTC+8 offset is valid");
    let now = chrono::Utc::now().with_timezone(&tz);
    let end = now.format("%Y%m%d").to_string();
    let start = (now - chrono::Duration::days(days_back.max(1)))
        .format("%Y%m%d")
        .to_string();
    (start, end)
}

/// Pull `(fields, items)` out of a tushare response body.
fn tushare_data(body: &serde_json::Value) -> Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
    let data = body
        .get("data")
        .ok_or_else(|| anyhow!("missing data in tushare response"))?;
    let fields = data
        .get("fields")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let items = data
        .get("items")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|r| r.as_array().cloned()).collect())
        .unwrap_or_default();
    Ok((fields, items))
}

/// One 大盘 row: (date, 上证%, 深成%, 主力净流入, 超大单, 大单, 中单, 小单).
type MarketFlowRow = (String, f64, f64, f64, f64, f64, f64, f64);

/// Render 大盘资金流 (moneyflow_mkt_dc) — last `days` trading days. Net amounts
/// arrive in 元 and are shown in 亿. 主力净流入 = 超大单 + 大单.
fn render_market_flow(fields: &[String], items: &[Vec<serde_json::Value>], days: usize) -> String {
    fn cell(row: &[serde_json::Value], i: Option<usize>) -> Option<&serde_json::Value> {
        i.and_then(|j| row.get(j))
    }
    let idx = |name: &str| fields.iter().position(|f| f == name);
    let (i_date, i_psh, i_psz, i_net, i_elg, i_lg, i_md, i_sm) = (
        idx("trade_date"),
        idx("pct_change_sh"),
        idx("pct_change_sz"),
        idx("net_amount"),
        idx("buy_elg_amount"),
        idx("buy_lg_amount"),
        idx("buy_md_amount"),
        idx("buy_sm_amount"),
    );
    if items.is_empty() {
        return "## 大盘资金流\n\n- 大盘资金流来源近期没有返回记录。这是来源覆盖缺口，不代表无资金流。\n"
            .to_string();
    }
    // (date, pct_sh, pct_sz, net, elg, lg, md, sm)
    let mut rows: Vec<MarketFlowRow> = items
        .iter()
        .map(|r| {
            (
                cell(r, i_date).map(value_text).unwrap_or_default(),
                value_f64(cell(r, i_psh)),
                value_f64(cell(r, i_psz)),
                value_f64(cell(r, i_net)),
                value_f64(cell(r, i_elg)),
                value_f64(cell(r, i_lg)),
                value_f64(cell(r, i_md)),
                value_f64(cell(r, i_sm)),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let rows = &rows[rows.len().saturating_sub(days)..];

    let mut out = format!("## 大盘资金流（近{}个交易日）\n\n", rows.len());
    out.push_str(
        "| 日期 | 上证涨跌% | 深成涨跌% | 主力净流入(亿) | 超大单(亿) | 大单(亿) | 中单(亿) | 小单(亿) |\n",
    );
    out.push_str(
        "|------|----------|----------|--------------|-----------|---------|---------|---------|\n",
    );
    for (date, psh, psz, net, elg, lg, md, sm) in rows {
        let d = if date.len() == 8 {
            format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
        } else {
            date.clone()
        };
        out.push_str(&format!(
            "| {} | {:+.2} | {:+.2} | {:+.1} | {:+.1} | {:+.1} | {:+.1} | {:+.1} |\n",
            d,
            psh,
            psz,
            net / 1e8,
            elg / 1e8,
            lg / 1e8,
            md / 1e8,
            sm / 1e8
        ));
    }
    out.push_str("\n_来源: 东方财富大盘资金流。单位亿元；主力净流入=超大单+大单，正为净流入。_\n");
    out
}

/// Render 行业/概念板块资金流 (moneyflow_ind_dc) for one day — top inflows and
/// top outflows by 主力净流入. Net amounts arrive in 元, shown in 亿.
fn render_board_flow(
    fields: &[String],
    items: &[Vec<serde_json::Value>],
    label: &str,
    date: &str,
    limit: usize,
) -> String {
    fn cell(row: &[serde_json::Value], i: Option<usize>) -> Option<&serde_json::Value> {
        i.and_then(|j| row.get(j))
    }
    let idx = |name: &str| fields.iter().position(|f| f == name);
    let (i_name, i_pct, i_net, i_lead) = (
        idx("name"),
        idx("pct_change"),
        idx("net_amount"),
        idx("buy_sm_amount_stock"),
    );
    let date_fmt = if date.len() == 8 {
        format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
    } else {
        date.to_string()
    };
    if items.is_empty() {
        return format!(
            "## {label}资金流（{date_fmt}）\n\n- 当日该板块来源没有返回记录。这是有效空结果或来源覆盖缺口。\n"
        );
    }
    // (net, name, pct, lead)
    let mut rows: Vec<(f64, String, f64, String)> = items
        .iter()
        .map(|r| {
            (
                value_f64(cell(r, i_net)),
                value_text_opt(cell(r, i_name)),
                value_f64(cell(r, i_pct)),
                value_text_opt(cell(r, i_lead)),
            )
        })
        .collect();
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = format!(
        "## {label}资金流（{date_fmt}）\n\n口径：东方财富主力净流入（超大单+大单），单位亿元。\n\n"
    );
    let top = &rows[..rows.len().min(limit)];
    push_board_table(&mut out, &format!("净流入 Top{}", top.len()), top);
    let mut bottom = rows[rows.len().saturating_sub(limit)..].to_vec();
    bottom.reverse();
    push_board_table(&mut out, &format!("净流出 Top{}", bottom.len()), &bottom);
    out.push_str("\n_来源: 东方财富板块资金流。单位亿元。_\n");
    out
}

fn push_board_table(out: &mut String, title: &str, rows: &[(f64, String, f64, String)]) {
    out.push_str(&format!(
        "### {title}\n\n| 板块 | 涨跌% | 主力净流入(亿) | 龙头 |\n|------|------|--------------|------|\n"
    ));
    for (net, name, pct, lead) in rows {
        out.push_str(&format!(
            "| {} | {:+.2} | {:+.1} | {} |\n",
            name,
            pct,
            net / 1e8,
            lead
        ));
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn northbound_renders_latest_day_top10_by_market() {
        let fields: Vec<String> = ["trade_date", "ts_code", "name", "market_type", "amount"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let items = vec![
            // Older day — must be excluded once a newer day is present.
            json_row(&["20260528", "600000.SH", "浦发银行", "1", "1.0e8"]),
            // Latest day: two 沪股通 (market_type 1) + one 深股通 (market_type 3).
            json_row(&["20260529", "600519.SH", "贵州茅台", "1", "5.0e8"]),
            json_row(&["20260529", "601318.SH", "中国平安", "1", "8.0e8"]),
            json_row(&["20260529", "000333.SZ", "美的集团", "3", "3.0e8"]),
        ];
        let out = render_northbound_top10(&fields, &items);

        assert!(out.contains("2026-05-29"));
        assert!(!out.contains("浦发银行"), "older trade day must be dropped");
        assert!(out.contains("### 沪股通十大成交活跃股"));
        assert!(out.contains("### 深股通十大成交活跃股"));
        // Sorted by amount desc within 沪股通: 中国平安(8亿) before 贵州茅台(5亿).
        let ping_an = out.find("中国平安").unwrap();
        let mao_tai = out.find("贵州茅台").unwrap();
        assert!(ping_an < mao_tai);
        assert!(out.contains("沪股通十大合计成交额 13.00 亿"));
        assert!(out.contains("美的集团"));
        assert!(out.contains("买卖净额自 2024-08-18 起停止披露"));
    }

    #[test]
    fn market_flow_renders_last_n_days_in_yi() {
        let fields: Vec<String> = [
            "trade_date",
            "pct_change_sh",
            "pct_change_sz",
            "net_amount",
            "buy_elg_amount",
            "buy_lg_amount",
            "buy_md_amount",
            "buy_sm_amount",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let row = |d: &str, net: f64| {
            vec![
                serde_json::Value::String(d.to_string()),
                serde_json::json!(-0.7),
                serde_json::json!(-1.8),
                serde_json::json!(net),
                serde_json::json!(net * 0.6),
                serde_json::json!(net * 0.4),
                serde_json::json!(0.0),
                serde_json::json!(0.0),
            ]
        };
        let items = vec![
            row("20260527", -10e8),
            row("20260528", -123333111808.0),
            row("20260529", 5e8),
        ];
        let out = render_market_flow(&fields, &items, 2);
        // Only the latest 2 days, latest last.
        assert!(!out.contains("2026-05-27"));
        assert!(out.contains("2026-05-28"));
        assert!(out.contains("2026-05-29"));
        // -123,333,111,808 元 -> -1233.3 亿
        assert!(out.contains("-1233.3"), "net should render in 亿: {out}");
        assert!(out.contains("东方财富大盘资金流"));
    }

    #[test]
    fn board_flow_splits_inflow_and_outflow_by_net() {
        let fields: Vec<String> = ["name", "pct_change", "net_amount", "buy_sm_amount_stock"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let row = |name: &str, pct: f64, net: f64, lead: &str| {
            vec![
                serde_json::Value::String(name.to_string()),
                serde_json::json!(pct),
                serde_json::json!(net),
                serde_json::Value::String(lead.to_string()),
            ]
        };
        let items = vec![
            row("通信设备", 3.07, 10.1e9, "中兴通讯"),
            row("白酒", 1.0, 2.0e9, "贵州茅台"),
            row("银行", -0.5, -3.0e9, "招商银行"),
            row("地产", -2.0, -8.0e9, "万科A"),
        ];
        let out = render_board_flow(&fields, &items, "行业板块", "20260529", 1);
        assert!(out.contains("行业板块资金流（2026-05-29）"));
        // Top inflow = 通信设备 (10.1e9 元 -> 101.0 亿); top outflow = 地产 (-80.0 亿).
        assert!(out.contains("通信设备"));
        assert!(out.contains("101.0"));
        assert!(out.contains("净流出 Top1"));
        assert!(out.contains("地产"));
        assert!(out.contains("-80.0"));
        // limit=1 must exclude the runner-up on each side.
        assert!(!out.contains("白酒"));
        assert!(!out.contains("银行"));
    }

    #[test]
    fn northbound_reports_empty_window_as_coverage_gap() {
        let fields: Vec<String> = ["trade_date", "ts_code", "name", "market_type", "amount"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = render_northbound_top10(&fields, &[]);
        assert!(out.contains("没有返回记录"));
        assert!(!out.contains("不可用"));
    }

    fn json_row(cells: &[&str; 5]) -> Vec<serde_json::Value> {
        vec![
            serde_json::Value::String(cells[0].to_string()),
            serde_json::Value::String(cells[1].to_string()),
            serde_json::Value::String(cells[2].to_string()),
            serde_json::json!(cells[3].parse::<i64>().unwrap()),
            serde_json::json!(cells[4].parse::<f64>().unwrap()),
        ]
    }

    #[test]
    fn eastmoney_secid_maps_a_share_suffixes() {
        assert_eq!(eastmoney_secid("600519.SH").unwrap(), "1.600519");
        assert_eq!(eastmoney_secid("000001.SZ").unwrap(), "0.000001");
        assert_eq!(eastmoney_secid("430047.BJ").unwrap(), "0.430047");
    }

    #[test]
    fn accumulator_computes_market_flow_totals() {
        let mut total = FlowAccumulator::default();
        total.add(&EastmoneyFlowRow {
            f6: Some(serde_json::json!(1000.0)),
            f62: Some(serde_json::json!(10.0)),
            f66: Some(serde_json::json!(4.0)),
            f72: Some(serde_json::json!(6.0)),
            f78: Some(serde_json::json!(-8.0)),
            f84: Some(serde_json::json!(-2.0)),
            f2: None,
            f3: None,
            f12: None,
            f14: None,
            f69: None,
            f75: None,
            f81: None,
            f87: None,
            f124: None,
            f184: None,
        });

        assert_eq!(total.amount, 1000.0);
        assert_eq!(total.main_net, 10.0);
        assert_eq!(fmt_pct_from_amount(total.main_net, total.amount), "1.00");
    }
}
