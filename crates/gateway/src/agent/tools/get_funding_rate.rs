use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_funding_rate";
const REQUEST_TIMEOUT_SECS: u64 = 20;

pub struct GetFundingRateTool {
    http: Client,
}

impl GetFundingRateTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
    }

    async fn fetch_binance(
        &self,
        symbol: &str,
        limit: u64,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let funding_url =
            format!("https://fapi.binance.com/fapi/v1/fundingRate?symbol={symbol}&limit={limit}");
        let oi_url = format!("https://fapi.binance.com/fapi/v1/openInterest?symbol={symbol}");
        let ls_url = format!(
            "https://fapi.binance.com/futures/data/globalLongShortAccountRatio?symbol={symbol}&period=1h&limit=8"
        );

        let funding_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&funding_url).send() => r?,
        };
        if !funding_resp.status().is_success() {
            bail!(
                "Binance fundingRate returned HTTP {}",
                funding_resp.status().as_u16()
            );
        }
        let funding_data: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = funding_resp.json() => r?,
        };

        let oi_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&oi_url).send() => r?,
        };
        if !oi_resp.status().is_success() {
            bail!(
                "Binance openInterest returned HTTP {}",
                oi_resp.status().as_u16()
            );
        }
        let oi_data: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = oi_resp.json() => r?,
        };

        let ls_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&ls_url).send() => r?,
        };
        if !ls_resp.status().is_success() {
            bail!(
                "Binance longShortRatio returned HTTP {}",
                ls_resp.status().as_u16()
            );
        }
        let ls_data: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = ls_resp.json() => r?,
        };

        let mut out = format!("## {symbol} · Binance Perpetual Futures\n\n");

        out.push_str(&format!("### Funding Rate (recent {limit})\n"));
        out.push_str("| Time               | Funding Rate | Mark Price   |\n");
        out.push_str("|-------------------|-------------|-------------|\n");
        if let Some(arr) = funding_data.as_array() {
            for item in arr {
                let time_ms = item
                    .get("fundingTime")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let rate = item
                    .get("fundingRate")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let mark = item
                    .get("markPrice")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                let dt = Utc
                    .timestamp_millis_opt(time_ms)
                    .single()
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "-".to_string());
                let rate_f: f64 = rate.parse().unwrap_or(0.0);
                let rate_pct = format!("{:+.4}%", rate_f * 100.0);
                let mark_f: f64 = mark.parse().unwrap_or(0.0);
                let mark_fmt = if mark_f > 0.0 {
                    format!("{:.2}", mark_f)
                } else {
                    mark.to_string()
                };
                out.push_str(&format!(
                    "| {:<18} | {:>12} | {:>12} |\n",
                    dt, rate_pct, mark_fmt
                ));
            }
        }
        out.push('\n');

        let oi_str = oi_data
            .get("openInterest")
            .and_then(|v| v.as_str())
            .unwrap_or("0");
        let oi_f: f64 = oi_str.parse().unwrap_or(0.0);
        let mark_latest = funding_data
            .as_array()
            .and_then(|a| a.last())
            .and_then(|item| item.get("markPrice"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let oi_usd = oi_f * mark_latest;
        out.push_str("### Open Interest\n");
        if oi_usd > 0.0 {
            out.push_str(&format!(
                "Current OI: {:.1} {} (${:.2}B)\n\n",
                oi_f,
                symbol.trim_end_matches(|c: char| c.is_alphabetic()
                    && c.is_uppercase()
                    && !"BTCETHBNBSOL".contains(c)),
                oi_usd / 1_000_000_000.0
            ));
        } else {
            out.push_str(&format!("Current OI: {oi_str}\n\n"));
        }

        out.push_str("### Long/Short Ratio (recent 8h)\n");
        out.push_str("| Time               | Long%  | Short% | L/S Ratio |\n");
        out.push_str("|-------------------|--------|--------|-----------|\n");
        if let Some(arr) = ls_data.as_array() {
            for item in arr {
                let ts = item.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
                let long_acc = item
                    .get("longAccount")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let short_acc = item
                    .get("shortAccount")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let ratio = item
                    .get("longShortRatio")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                let dt = Utc
                    .timestamp_millis_opt(ts)
                    .single()
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "-".to_string());
                let long_f: f64 = long_acc.parse().unwrap_or(0.0);
                let short_f: f64 = short_acc.parse().unwrap_or(0.0);
                let ratio_f: f64 = ratio.parse().unwrap_or(0.0);
                out.push_str(&format!(
                    "| {:<18} | {:>6} | {:>6} | {:>9} |\n",
                    dt,
                    format!("{:.1}%", long_f * 100.0),
                    format!("{:.1}%", short_f * 100.0),
                    format!("{:.3}", ratio_f),
                ));
            }
        }
        out.push('\n');
        out.push_str("_Source: Binance USDM Futures_\n");
        Ok(out)
    }

    async fn fetch_okx(&self, symbol: &str, cancel: &CancellationToken) -> Result<String> {
        let funding_url = format!("https://www.okx.com/api/v5/public/funding-rate?instId={symbol}");
        let oi_url = format!("https://www.okx.com/api/v5/public/open-interest?instId={symbol}");

        let funding_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&funding_url).send() => r?,
        };
        if !funding_resp.status().is_success() {
            bail!(
                "OKX funding-rate returned HTTP {}",
                funding_resp.status().as_u16()
            );
        }
        let funding_body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = funding_resp.json() => r?,
        };

        let oi_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&oi_url).send() => r?,
        };
        if !oi_resp.status().is_success() {
            bail!(
                "OKX open-interest returned HTTP {}",
                oi_resp.status().as_u16()
            );
        }
        let oi_body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = oi_resp.json() => r?,
        };

        let mut out = format!("## {symbol} · OKX Perpetual Futures\n\n");

        let empty_arr = serde_json::Value::Array(vec![]);
        let funding_arr = funding_body
            .get("data")
            .and_then(|v| v.as_array())
            .unwrap_or(
                funding_body
                    .as_array()
                    .unwrap_or(empty_arr.as_array().unwrap()),
            );

        out.push_str("### Current Funding Rate\n");
        if let Some(item) = funding_arr.first() {
            let rate = item
                .get("fundingRate")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let next_rate = item
                .get("nextFundingRate")
                .and_then(|v| v.as_str())
                .unwrap_or("0");
            let next_time_ms = item
                .get("nextFundingTime")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let mark = item
                .get("markPrice")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let rate_f: f64 = rate.parse().unwrap_or(0.0);
            let next_rate_f: f64 = next_rate.parse().unwrap_or(0.0);
            let next_dt = Utc
                .timestamp_millis_opt(next_time_ms)
                .single()
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "-".to_string());
            out.push_str(&format!("- Current rate: {:+.4}%\n", rate_f * 100.0));
            out.push_str(&format!(
                "- Next rate: {:+.4}% ({})\n",
                next_rate_f * 100.0,
                next_dt
            ));
            out.push_str(&format!("- Mark price: {mark}\n"));
        }
        out.push('\n');

        let oi_arr = oi_body.get("data").and_then(|v| v.as_array());
        out.push_str("### Open Interest\n");
        if let Some(arr) = oi_arr {
            if let Some(item) = arr.first() {
                let oi = item.get("oi").and_then(|v| v.as_str()).unwrap_or("-");
                let oi_ccy = item.get("oiCcy").and_then(|v| v.as_str()).unwrap_or("-");
                out.push_str(&format!("Current OI: {oi} {oi_ccy}\n"));
            }
        }
        out.push('\n');
        out.push_str("_Source: OKX_\n");
        Ok(out)
    }
}

#[async_trait]
impl ToolHandler for GetFundingRateTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Fetch perpetual futures funding rates, open interest, and long/short positioning where available. \
                Use when the task needs leverage, crowding, or derivatives-flow context for crypto assets. \
                This is derivatives market evidence, not a standalone long/short signal; combine with spot trend, liquidity, and event context. \
                Supports Binance (default) and OKX. \
                symbol format: \"BTCUSDT\" for Binance, \"BTC-USD-SWAP\" for OKX."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Trading pair symbol. e.g. 'BTCUSDT' for Binance, 'BTC-USD-SWAP' for OKX."
                    },
                    "exchange": {
                        "type": "string",
                        "enum": ["binance", "okx"],
                        "description": "Exchange to query (default: binance)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of recent funding rate records (default 10, max 100). Binance only."
                    }
                },
                "required": ["symbol"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let symbol = args
            .get("symbol")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'symbol' argument"))?
            .trim()
            .to_uppercase();
        let exchange = args
            .get("exchange")
            .and_then(|v| v.as_str())
            .unwrap_or("binance");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 100))
            .unwrap_or(10);

        match exchange {
            "okx" => self.fetch_okx(&symbol, &cancel).await,
            _ => self.fetch_binance(&symbol, limit, &cancel).await,
        }
    }
}
