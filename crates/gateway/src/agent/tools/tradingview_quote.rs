//! `tradingview_quote` — real-time-ish snapshot quotes via TradingView's
//! undocumented scanner endpoint.
//!
//! POST https://scanner.tradingview.com/{market}/scan
//! Body: `{ symbols: { tickers: ["NASDAQ:NVDA", ...] }, columns: [...] }`
//! Response: `{ data: [ { s: "NASDAQ:NVDA", d: [name, close, change, ...] } ] }`
//!
//! No auth required for delayed quotes; real-time would need session cookies.
//! Delayed is fine for our purposes — investment-research, not day-trading.
//! Endpoint may break without notice (it's an internal API).

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "market_quote";
const ENDPOINT_TPL: &str = "https://scanner.tradingview.com/{market}/scan";
const REQUEST_TIMEOUT_SECS: u64 = 15;
const MAX_TICKERS: usize = 25;

const COLUMNS: &[&str] = &[
    "name",        // symbol short
    "description", // company name
    "close",       // last/close
    "change",      // % change
    "change_abs",  // absolute change
    "volume",
    "market_cap_basic",
    "high",
    "low",
    "open",
    "Recommend.All", // -1..1 technical rating
];

pub struct TradingViewQuoteTool {
    http: Client,
}

impl TradingViewQuoteTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            // TradingView's edge sometimes requires a real-ish UA.
            .user_agent("Mozilla/5.0 (compatible; leek-research-agent/0.1)")
            .build()?;
        Ok(Self { http })
    }
}

#[async_trait]
impl ToolHandler for TradingViewQuoteTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Snapshot quotes (close / change / volume / market cap / high-low / \
                 technical rating) for one or more tickers. \
                 Use this when web_search's snippet isn't precise enough or when you \
                 need to compare multiple symbols side-by-side. Tickers must be in \
                 `EXCHANGE:SYMBOL` format: \
                 `NASDAQ:NVDA`, `NYSE:BRK.B`, `AMEX:SPY`, `BATS:VTI`, `HKEX:0700`, \
                 `LSE:HSBA`, `SSE:600519`, `SZSE:000001`, `BINANCE:BTCUSDT`. For US \
                 stocks set `market='america'` (default). For non-US: `china`, \
                 `hongkong`, `india`, `japan`, `crypto`, `forex`, `germany`. Returns a \
                 markdown table. Quotes may be 15-min delayed."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tickers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of EXCHANGE:SYMBOL strings (max 25). Order is preserved in the output."
                    },
                    "market": {
                        "type": "string",
                        "description": "Market segment (default 'america'). Common values: america, china, hongkong, india, japan, germany, uk, canada, australia, crypto, forex.",
                    }
                },
                "required": ["tickers"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let tickers: Vec<String> = args
            .get("tickers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if tickers.is_empty() {
            bail!("'tickers' must be a non-empty array of EXCHANGE:SYMBOL strings");
        }
        if tickers.len() > MAX_TICKERS {
            bail!("too many tickers ({}, max {MAX_TICKERS})", tickers.len());
        }
        for t in &tickers {
            if !t.contains(':') {
                bail!(
                    "ticker `{t}` missing exchange prefix — use `EXCHANGE:SYMBOL` \
                     (e.g. `NASDAQ:NVDA`)"
                );
            }
        }

        let market = args
            .get("market")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "america".into());
        if !is_valid_market(&market) {
            bail!("unknown TradingView market segment `{market}`");
        }

        let url = ENDPOINT_TPL.replace("{market}", &market);
        let payload = serde_json::json!({
            "symbols": { "tickers": tickers, "query": { "types": [] } },
            "columns": COLUMNS,
        });

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before tradingview request"),
            r = self.http.post(&url).json(&payload).send() => r?,
        };
        let status = resp.status();
        if !status.is_success() {
            let body_preview = resp.text().await.unwrap_or_default();
            bail!(
                "tradingview scanner returned HTTP {} ({})",
                status.as_u16(),
                body_preview.chars().take(200).collect::<String>()
            );
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during tradingview parse"),
            r = resp.json() => r?,
        };

        let rows = body
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("tradingview response missing 'data' array"))?;
        if rows.is_empty() {
            return Ok(format!(
                "[market_quote: no rows returned for {} tickers in market '{market}'. \
                 Verify the EXCHANGE:SYMBOL format.]",
                tickers.len()
            ));
        }

        // Build a lookup from symbol → values to preserve user's input order.
        let mut by_symbol: std::collections::HashMap<String, Vec<serde_json::Value>> =
            std::collections::HashMap::new();
        for row in rows {
            let Some(s) = row.get("s").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(d) = row.get("d").and_then(|v| v.as_array()) else {
                continue;
            };
            by_symbol.insert(s.to_string(), d.clone());
        }

        let mut out = String::new();
        out.push_str(&format!(
            "# Market quote · {market} · {} symbols\n\n",
            tickers.len()
        ));
        out.push_str(
            "| symbol            | name                  | close   | %chg   | abs    | volume        | mkt_cap        | tech |\n",
        );
        out.push_str(
            "|-------------------|-----------------------|---------|--------|--------|---------------|----------------|------|\n",
        );
        for t in &tickers {
            let Some(d) = by_symbol.get(t) else {
                out.push_str(&format!("| {t} | — | — | — | — | — | — | — |\n"));
                continue;
            };
            let cell = |i: usize| match d.get(i) {
                Some(serde_json::Value::Number(n)) => fmt_number(n.as_f64().unwrap_or(f64::NAN), i),
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Null) | None => "—".into(),
                Some(other) => other.to_string(),
            };
            // COLUMNS index: 0=name, 1=description, 2=close, 3=change, 4=change_abs,
            //               5=volume, 6=market_cap_basic, 7=high, 8=low, 9=open, 10=Recommend.All
            out.push_str(&format!(
                "| {:<17} | {:<21} | {:>7} | {:>6} | {:>6} | {:>13} | {:>14} | {:>4} |\n",
                t,
                truncate(&cell(1), 21),
                cell(2),
                cell(3),
                cell(4),
                cell(5),
                cell(6),
                cell(10),
            ));
        }
        out.push_str("\n_Source: scanner.tradingview.com (delayed; undocumented internal API)._\n");
        Ok(out)
    }
}

fn is_valid_market(m: &str) -> bool {
    matches!(
        m,
        "america"
            | "china"
            | "hongkong"
            | "india"
            | "japan"
            | "germany"
            | "uk"
            | "canada"
            | "australia"
            | "crypto"
            | "forex"
            | "global"
            | "egypt"
            | "indonesia"
            | "korea"
            | "taiwan"
            | "vietnam"
            | "italy"
            | "france"
            | "spain"
            | "switzerland"
            | "brazil"
    )
}

/// Format numeric values per column. Volume / market cap get integer-style;
/// everything else gets up to 2 decimals.
fn fmt_number(n: f64, col_idx: usize) -> String {
    if !n.is_finite() {
        return "—".into();
    }
    match col_idx {
        // volume, market_cap_basic — large ints, group with k/M/B suffix.
        5 | 6 => fmt_compact(n),
        // change %, change_abs, technical rating — keep sign + decimals.
        3 | 4 | 10 => format!("{n:+.2}"),
        // close, high, low, open — 2 decimals.
        _ => format!("{n:.2}"),
    }
}

fn fmt_compact(n: f64) -> String {
    let abs = n.abs();
    if abs >= 1.0e9 {
        format!("{:.2}B", n / 1.0e9)
    } else if abs >= 1.0e6 {
        format!("{:.2}M", n / 1.0e6)
    } else if abs >= 1.0e3 {
        format!("{:.2}k", n / 1.0e3)
    } else {
        format!("{n:.0}")
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let mut t: String = chars
            .into_iter()
            .take(max_chars.saturating_sub(1))
            .collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_known_markets() {
        assert!(is_valid_market("america"));
        assert!(is_valid_market("hongkong"));
        assert!(is_valid_market("crypto"));
        assert!(!is_valid_market("mars"));
    }

    #[test]
    fn fmt_compact_units() {
        assert_eq!(fmt_compact(1234.0), "1.23k");
        assert_eq!(fmt_compact(1_500_000.0), "1.50M");
        assert_eq!(fmt_compact(2_500_000_000.0), "2.50B");
        assert_eq!(fmt_compact(42.0), "42");
    }

    #[test]
    fn fmt_number_signs_changes() {
        assert_eq!(fmt_number(2.34, 3), "+2.34"); // change %
        assert_eq!(fmt_number(-1.5, 4), "-1.50"); // change_abs
        assert_eq!(fmt_number(198.45, 2), "198.45"); // close
    }

    #[test]
    fn truncate_chinese_chars() {
        let s = "贵州茅台酒股份有限公司";
        let t = truncate(s, 5);
        assert!(t.chars().count() <= 5);
        assert!(t.ends_with('…'));
    }
}
