use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{ToolContext, ToolHandler, data_provider_tokens};

const TOOL_NAME: &str = "get_a_share_market_snapshot";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const MAX_TICKERS: usize = 20;
const EASTMONEY_UT: &str = "b2884a393a59ad64002292a3e90d46a5";
const EASTMONEY_QUOTE_FIELDS: &str =
    "f12,f14,f2,f3,f4,f5,f6,f8,f9,f20,f21,f23,f15,f16,f17,f18,f124,f152";
const EASTMONEY_QUOTE_URLS: [(&str, &str); 2] = [
    (
        "https://push2.eastmoney.com/api/qt/ulist.np/get",
        "eastmoney_push2",
    ),
    (
        "https://push2delay.eastmoney.com/api/qt/ulist.np/get",
        "eastmoney_push2delay",
    ),
];

pub struct GetAShareMarketSnapshotTool {
    http: Client,
}

impl GetAShareMarketSnapshotTool {
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

    async fn fetch_tushare_latest_close(
        &self,
        token: &str,
        ts_codes: &[String],
        cancel: &CancellationToken,
    ) -> Result<String> {
        let mut out = "## A股市场快照 · Tushare 最近收盘\n\n".to_string();
        let mut empty_codes = Vec::new();
        out.push_str(
            "| 代码 | 名称 | 交易日 | 最近收盘 | 涨跌幅% | 成交量(手) | 成交额(千元) | 换手率% | PE(TTM) | PB | 总市值(万元) | 涨跌停价 |\n",
        );
        out.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");

        for code in ts_codes {
            let basic = self
                .tushare_post(
                    token,
                    "stock_basic",
                    serde_json::json!({"ts_code": code}),
                    "ts_code,name,industry,market,list_date",
                    cancel,
                )
                .await?;
            let quote = self
                .tushare_post(
                    token,
                    "daily",
                    serde_json::json!({"ts_code": code, "limit": 1}),
                    "ts_code,trade_date,close,pct_chg,vol,amount",
                    cancel,
                )
                .await?;
            if quote.rows.is_empty() {
                empty_codes.push(code.clone());
                out.push_str(&format!(
                    "| {} | {} | 无返回记录 |  |  |  |  |  |  |  |  |  |\n",
                    code,
                    first_text(&basic, "name")
                ));
                continue;
            }
            let valuation = self
                .tushare_post(
                    token,
                    "daily_basic",
                    serde_json::json!({"ts_code": code, "limit": 1}),
                    "ts_code,trade_date,turnover_rate,pe_ttm,pb,total_mv,circ_mv",
                    cancel,
                )
                .await?;
            let limit = self
                .tushare_post(
                    token,
                    "stk_limit",
                    serde_json::json!({"ts_code": code, "limit": 1}),
                    "ts_code,trade_date,up_limit,down_limit",
                    cancel,
                )
                .await
                .ok();

            let name = first_text(&basic, "name");
            let trade_date = first_text(&quote, "trade_date");
            let close = first_text(&quote, "close");
            let pct = first_text(&quote, "pct_chg");
            let vol = first_text(&quote, "vol");
            let amount = first_text(&quote, "amount");
            let turnover = first_text(&valuation, "turnover_rate");
            let pe = first_text(&valuation, "pe_ttm");
            let pb = first_text(&valuation, "pb");
            let total_mv = first_text(&valuation, "total_mv");
            let limits = limit
                .as_ref()
                .map(|table| {
                    format!(
                        "{}/{}",
                        first_text(table, "up_limit"),
                        first_text(table, "down_limit")
                    )
                })
                .unwrap_or_else(|| "n/a".to_string());
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                code, name, trade_date, close, pct, vol, amount, turnover, pe, pb, total_mv, limits
            ));
        }
        if !empty_codes.is_empty() {
            out.push_str(&format!(
                "\n_{} 在最近收盘口径下没有返回记录。这是有效空结果，不代表零成交或不在其他来源覆盖中。_\n",
                empty_codes.join(", ")
            ));
        }
        out.push_str("\n_来源: 配置的官方日频行情/估值来源；成交量与成交额是最近交易日收盘口径，不是盘中实时成交。_\n");
        Ok(out)
    }

    async fn fetch_eastmoney_realtime(
        &self,
        ts_codes: &[String],
        cancel: &CancellationToken,
    ) -> Result<String> {
        let secids = ts_codes
            .iter()
            .map(|code| eastmoney_secid(code))
            .collect::<Result<Vec<_>>>()?
            .join(",");
        let mut last_error = None;

        for (url, source) in EASTMONEY_QUOTE_URLS {
            match self
                .fetch_eastmoney_realtime_from(url, source, &secids, cancel)
                .await
            {
                Ok(out) => return Ok(out),
                Err(err) if is_abort_error(&err, cancel) => return Err(err),
                Err(err) => last_error = Some(err),
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Eastmoney quote source unavailable")))
    }

    async fn fetch_eastmoney_realtime_from(
        &self,
        url: &str,
        source: &str,
        secids: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let request = self
            .http
            .get(url)
            .query(&[
                ("fltt", "2"),
                ("secids", secids),
                ("fields", EASTMONEY_QUOTE_FIELDS),
                ("ut", EASTMONEY_UT),
            ])
            .build()?;
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney quote request"),
            result = self.http.execute(request) => result?,
        };
        let status = response.status();
        if !status.is_success() {
            bail!("Eastmoney quote returned HTTP {status}");
        }
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before Eastmoney quote response body"),
            result = response.text() => result?,
        };
        let body: EastmoneyQuoteResponse = serde_json::from_str(&text).map_err(|err| {
            anyhow!(
                "Eastmoney quote returned invalid JSON: {err}; sample: {}",
                compact_text(&text, 160)
            )
        })?;
        if body.rc != Some(0) {
            bail!(
                "Eastmoney quote returned rc={:?}: {}",
                body.rc,
                body.message_text().unwrap_or("unknown error")
            );
        }

        let rows = body.data.and_then(|data| data.diff).unwrap_or_default();
        let source_label = eastmoney_quote_source_label(source);
        let mut out = format!("## A股市场快照 · {source_label}\n\n");
        out.push_str(
            "| 代码 | 名称 | 更新时间 | 最新价 | 涨跌幅% | 涨跌额 | 今开 | 最高 | 最低 | 昨收 | 成交量(手) | 成交额(元) | 换手率% | PE(动) | PB | 总市值(元) |\n",
        );
        out.push_str(
            "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
        );

        if rows.is_empty() {
            out.push_str("| - | - | - | - | - | - | - | - | - | - | - | - | - | - | - | - |\n");
            out.push_str(
                "\n_当前公开行情来源没有返回记录；这是有效空结果，不代表该股票不交易。_\n",
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
                    value_text_opt(row.f4.as_ref()),
                    value_text_opt(row.f17.as_ref()),
                    value_text_opt(row.f15.as_ref()),
                    value_text_opt(row.f16.as_ref()),
                    value_text_opt(row.f18.as_ref()),
                    value_text_opt(row.f5.as_ref()),
                    value_text_opt(row.f6.as_ref()),
                    value_text_opt(row.f8.as_ref()),
                    value_text_opt(row.f9.as_ref()),
                    value_text_opt(row.f23.as_ref()),
                    value_text_opt(row.f20.as_ref())
                ));
            }
        }
        out.push_str(&format!(
            "\n_来源: 东方财富公开行情；当前口径：{source_label}。按公开延迟/近实时快照处理，不要与 Tushare 最近收盘字段混作同一口径。_\n"
        ));
        Ok(out)
    }
}

fn eastmoney_quote_source_label(source: &str) -> &'static str {
    match source {
        "eastmoney_push2" => "东方财富盘中行情",
        "eastmoney_push2delay" => "东方财富延迟行情",
        _ => "东方财富公开行情",
    }
}

#[async_trait]
impl ToolHandler for GetAShareMarketSnapshotTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch a compact A-share market snapshot for one or more stocks. `latest_close` uses the configured official daily close/valuation source for closing data, market cap, and daily limit prices. `realtime` uses public quote data for the current/near-realtime trading snapshot. Use `both` when the user needs to compare current price against the latest official close. Always preserve source/freshness: official daily data can lag same-day close updates, while public quote data may be delayed or temporarily unavailable."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ts_codes": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "A-share codes, max 20, e.g. [\"600519.SH\", \"000858.SZ\"]"
                    },
                    "freshness": {
                        "type": "string",
                        "enum": ["latest_close", "realtime", "both"],
                        "description": "Default latest_close. Use realtime for current/near-realtime quote only; use both to show official latest close plus current public quote snapshot."
                    }
                },
                "required": ["ts_codes"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let requested_ts_codes = args
            .get("ts_codes")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("ts_codes is required"))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_uppercase()))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let truncated_count = requested_ts_codes.len().saturating_sub(MAX_TICKERS);
        let ts_codes = requested_ts_codes
            .into_iter()
            .take(MAX_TICKERS)
            .collect::<Vec<_>>();
        if ts_codes.is_empty() {
            bail!("ts_codes must contain at least one A-share code");
        }
        let freshness = args
            .get("freshness")
            .and_then(|v| v.as_str())
            .unwrap_or("latest_close");
        let include_latest_close = matches!(freshness, "latest_close" | "both");
        let include_realtime = matches!(freshness, "realtime" | "both");
        if !include_latest_close && !include_realtime {
            bail!("freshness must be one of latest_close, realtime, or both");
        }

        let mut sections = Vec::new();
        if include_latest_close {
            let latest_close = match data_provider_tokens::tushare_token(ctx).await {
                Ok(token) => {
                    self.fetch_tushare_latest_close(&token, &ts_codes, &cancel)
                        .await
                }
                Err(err) => Err(err),
            };
            push_section_result(
                &mut sections,
                "A股市场快照",
                "tushare_latest_close",
                latest_close,
                &cancel,
            )?;
        }
        if include_realtime {
            let realtime = self.fetch_eastmoney_realtime(&ts_codes, &cancel).await;
            push_section_result(
                &mut sections,
                "A股市场快照",
                "eastmoney_realtime_quote",
                realtime,
                &cancel,
            )?;
        }

        let mut out = sections.join("\n");
        if truncated_count > 0 {
            out.push_str(&format!(
                "\n_Limit note: tool returned the first {MAX_TICKERS} tickers and omitted {truncated_count}; split the request if the omitted names matter._"
            ));
        }
        Ok(out)
    }
}

#[derive(Deserialize)]
struct EastmoneyQuoteResponse {
    rc: Option<i64>,
    data: Option<EastmoneyQuoteData>,
    msg: Option<String>,
    message: Option<String>,
}

impl EastmoneyQuoteResponse {
    fn message_text(&self) -> Option<&str> {
        clean_opt(self.message.as_deref()).or_else(|| clean_opt(self.msg.as_deref()))
    }
}

#[derive(Deserialize)]
struct EastmoneyQuoteData {
    diff: Option<Vec<EastmoneyQuoteRow>>,
}

#[derive(Deserialize)]
struct EastmoneyQuoteRow {
    f2: Option<serde_json::Value>,
    f3: Option<serde_json::Value>,
    f4: Option<serde_json::Value>,
    f5: Option<serde_json::Value>,
    f6: Option<serde_json::Value>,
    f8: Option<serde_json::Value>,
    f9: Option<serde_json::Value>,
    f12: Option<serde_json::Value>,
    f14: Option<serde_json::Value>,
    f15: Option<serde_json::Value>,
    f16: Option<serde_json::Value>,
    f17: Option<serde_json::Value>,
    f18: Option<serde_json::Value>,
    f20: Option<serde_json::Value>,
    f23: Option<serde_json::Value>,
    f124: Option<serde_json::Value>,
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

fn first_text(table: &Table, field: &str) -> String {
    table
        .rows
        .first()
        .map(|row| table.text(row, field))
        .unwrap_or_default()
}

fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn value_text_opt(v: Option<&serde_json::Value>) -> String {
    v.map(value_text).unwrap_or_default()
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
    bail!("Eastmoney realtime quote expects A-share ts_code with .SH, .SZ, or .BJ suffix");
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

fn push_section_result(
    sections: &mut Vec<String>,
    title: &'static str,
    source: &'static str,
    result: Result<String>,
    cancel: &CancellationToken,
) -> Result<()> {
    match result {
        Ok(section) => sections.push(section),
        Err(err) if is_abort_error(&err, cancel) => return Err(err),
        Err(err) => sections.push(unavailable_section(title, source, &err)),
    }
    Ok(())
}

fn unavailable_section(title: &str, source: &str, err: &anyhow::Error) -> String {
    format!(
        "## {title} · {} · 来源不可用\n\n- 来源不可用：{}。这代表覆盖/访问缺口，不是有效空结果。\n",
        market_snapshot_source_label(source),
        compact_text(&err.to_string(), 180)
    )
}

fn market_snapshot_source_label(source: &str) -> &'static str {
    match source {
        "tushare_latest_close" => "最近收盘",
        "eastmoney_realtime_quote" => "盘中公开行情",
        _ => "行情来源",
    }
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

fn clean_opt(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty() && *s != "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_text_returns_empty_for_valid_empty_table() {
        let table = Table {
            fields: vec!["close".into()],
            rows: Vec::new(),
        };
        assert_eq!(first_text(&table, "close"), "");
    }

    #[test]
    fn eastmoney_secid_maps_a_share_suffixes() {
        assert_eq!(eastmoney_secid("600519.SH").unwrap(), "1.600519");
        assert_eq!(eastmoney_secid("000001.SZ").unwrap(), "0.000001");
        assert_eq!(eastmoney_secid("430047.BJ").unwrap(), "0.430047");
    }
}
