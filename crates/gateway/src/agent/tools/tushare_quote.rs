//! `tushare_quote` — Chinese A-share OHLCV via Tushare Pro API.
//!
//! Tushare is the de-facto open data source for CN equities. Token is read
//! from the `TUSHARE_TOKEN` env var (never hard-coded). Endpoint:
//!   POST https://api.tushare.pro
//! Body: `{api_name, token, params, fields}` — we wrap the `daily` interface.
//!
//! The model passes a `ts_code` like `000001.SZ` (Ping An Bank) or
//! `600519.SH` (Kweichow Moutai) plus an optional day range. We return a
//! markdown table sorted oldest→newest so candle context reads naturally.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{Datelike, NaiveDate, Utc};
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "tushare_quote";
const ENDPOINT: &str = "https://api.tushare.pro";
const FIELDS: &str = "ts_code,trade_date,open,high,low,close,pre_close,change,pct_chg,vol,amount";
const DEFAULT_DAYS: u32 = 5;
const MAX_DAYS: u32 = 60;
const REQUEST_TIMEOUT_SECS: u64 = 20;

pub struct TushareQuoteTool {
    http: Client,
}

impl TushareQuoteTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
    }
}

#[async_trait]
impl ToolHandler for TushareQuoteTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Fetch daily OHLCV (open/high/low/close/volume) for a Chinese A-share \
                 from Tushare Pro. Use this for any A-share or Chinese index \
                 question — codex's web_search has poor coverage on CN equities. \
                 Symbol format is `<ticker>.<exchange>`: `.SH` for Shanghai \
                 (e.g. 600519.SH = Moutai), `.SZ` for Shenzhen (e.g. 000001.SZ = \
                 Ping An Bank, 300750.SZ = CATL), `.BJ` for Beijing. Returns a \
                 markdown table of the most recent N trading days."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ts_code": {
                        "type": "string",
                        "description": "Tushare ts_code, e.g. '000001.SZ', '600519.SH', '300750.SZ'."
                    },
                    "days": {
                        "type": "integer",
                        "description": "Number of recent trading days to return (default 5, max 60). Tushare returns by trading day, not calendar day."
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
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let token = std::env::var("TUSHARE_TOKEN").map_err(|_| {
            anyhow!(
                "TUSHARE_TOKEN env var not set — A-share quotes unavailable. \
                 Get a free token at https://tushare.pro/register"
            )
        })?;
        let ts_code = args
            .get("ts_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'ts_code' argument"))?
            .trim()
            .to_uppercase();
        if !is_valid_ts_code(&ts_code) {
            bail!(
                "invalid ts_code `{ts_code}` — expected format like '000001.SZ' / '600519.SH'"
            );
        }
        let days = args
            .get("days")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).min(MAX_DAYS).max(1))
            .unwrap_or(DEFAULT_DAYS);

        // Tushare daily returns up to a configurable window; we send a wide
        // calendar window (days * 2 to absorb weekends/holidays) and trim to
        // the latest N rows after sorting.
        let end = Utc::now().date_naive();
        let lookback_calendar = days as i64 * 2 + 7;
        let start = end - chrono::Duration::days(lookback_calendar);

        let payload = serde_json::json!({
            "api_name": "daily",
            "token": token,
            "params": {
                "ts_code": ts_code,
                "start_date": yyyymmdd(start),
                "end_date": yyyymmdd(end),
            },
            "fields": FIELDS,
        });

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before tushare request"),
            r = self.http.post(ENDPOINT).json(&payload).send() => r?,
        };
        let status = resp.status();
        if !status.is_success() {
            bail!("tushare returned HTTP {}", status.as_u16());
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during tushare parse"),
            r = resp.json() => r?,
        };

        // Tushare error shape: { code: <non-zero>, msg: "..." }
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown tushare error");
            bail!("tushare error (code={code}): {msg}");
        }
        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("tushare response missing 'data'"))?;
        let fields: Vec<String> = data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let items: Vec<&Vec<serde_json::Value>> = data
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|row| row.as_array()).collect())
            .unwrap_or_default();
        if items.is_empty() {
            return Ok(format!(
                "[tushare_quote: no rows returned for {ts_code} in last {days} days. \
                 Verify the ts_code format and that the symbol is currently trading.]"
            ));
        }

        // Find column indices we care about for the table.
        let idx = |name: &str| fields.iter().position(|f| f == name);
        let i_date = idx("trade_date");
        let i_open = idx("open");
        let i_high = idx("high");
        let i_low = idx("low");
        let i_close = idx("close");
        let i_pct = idx("pct_chg");
        let i_vol = idx("vol");

        let mut rows: Vec<Vec<String>> = items
            .iter()
            .map(|row| {
                let cell = |i: Option<usize>| -> String {
                    i.and_then(|j| row.get(j))
                        .map(|v| match v {
                            serde_json::Value::Null => String::new(),
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default()
                };
                vec![
                    fmt_date(&cell(i_date)),
                    cell(i_open),
                    cell(i_high),
                    cell(i_low),
                    cell(i_close),
                    cell(i_pct),
                    cell(i_vol),
                ]
            })
            .collect();
        // Sort oldest → newest by trade_date string (yyyy-mm-dd is comparable).
        rows.sort_by(|a, b| a[0].cmp(&b[0]));
        // Keep only the latest `days` rows.
        if rows.len() > days as usize {
            let drop = rows.len() - days as usize;
            rows.drain(..drop);
        }

        let mut out = String::new();
        out.push_str(&format!("# {ts_code} · daily (latest {})\n\n", rows.len()));
        out.push_str("| date       | open    | high    | low     | close   | %chg   | vol(手) |\n");
        out.push_str("|------------|---------|---------|---------|---------|--------|---------|\n");
        for r in &rows {
            out.push_str(&format!(
                "| {:<10} | {:>7} | {:>7} | {:>7} | {:>7} | {:>6} | {:>8} |\n",
                r[0], r[1], r[2], r[3], r[4], r[5], r[6]
            ));
        }
        out.push_str("\n_Source: Tushare Pro (`daily`)_\n");
        Ok(out)
    }
}

fn is_valid_ts_code(s: &str) -> bool {
    // <digits>.<2-letter exchange>
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 2 {
        return false;
    }
    if parts[0].is_empty() || !parts[0].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    matches!(parts[1], "SH" | "SZ" | "BJ")
}

fn yyyymmdd(d: NaiveDate) -> String {
    format!("{:04}{:02}{:02}", d.year(), d.month(), d.day())
}

/// `20260501` → `2026-05-01` for readability in the table. Returns input
/// untouched if it doesn't look like an 8-digit Tushare date.
fn fmt_date(s: &str) -> String {
    if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ts_code_format() {
        assert!(is_valid_ts_code("000001.SZ"));
        assert!(is_valid_ts_code("600519.SH"));
        assert!(is_valid_ts_code("300750.SZ"));
        assert!(is_valid_ts_code("835174.BJ"));
    }

    #[test]
    fn rejects_malformed_ts_code() {
        assert!(!is_valid_ts_code("NVDA"));
        assert!(!is_valid_ts_code("600519"));
        assert!(!is_valid_ts_code("600519.NYSE"));
        assert!(!is_valid_ts_code(".SH"));
        assert!(!is_valid_ts_code("abc.SZ"));
    }

    #[test]
    fn formats_yyyymmdd() {
        let d = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        assert_eq!(yyyymmdd(d), "20260501");
    }

    #[test]
    fn humanizes_date() {
        assert_eq!(fmt_date("20260501"), "2026-05-01");
        assert_eq!(fmt_date("2026-05-01"), "2026-05-01");
        assert_eq!(fmt_date(""), "");
    }
}
