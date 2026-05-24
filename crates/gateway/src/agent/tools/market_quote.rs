//! `market_quote` — snapshot price + change + volume for one A-share
//! symbol. Vendor-neutral by name and field set; ARCHITECTURE §12.1.
//!
//! Fallback chain: primary vendor → fallback vendor → structured error.
//! The first vendor to return a non-error response wins; recoverable
//! errors fall through, fatal errors short-circuit.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorQuote, VendorRegistry};

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "market_quote".into(),
        description: "Fetch a quote snapshot for one Chinese A-share equity (Shanghai \
             or Shenzhen). Returns latest price, day change, percent change, day \
             open/high/low, volume, turnover, and a freshness tag. Accepts symbols \
             in any of these forms: '600519.SH' (dotted, preferred), '600519' (bare \
             6-digit code; exchange inferred), or 'sh600519' (prefixed). Use when \
             the user names a stock and you need a live snapshot before deeper \
             analysis. For multi-day OHLCV use `get_candlesticks` instead."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol. Examples: '600519.SH' (贵州茅台), \
                                    '000001.SZ' (平安银行), '300750' (宁德时代 — bare \
                                    codes auto-resolve their exchange)."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "行情快照",
        result: ResultArtifact::Card("market_quote"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            format!("行情快照 · {}", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("market_quote: missing required string argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("market_quote: {e}")),
    };

    // Fallback chain: primary token-backed source → public token-free
    // source. Each entry is a (tag, trait obj) pair — the tag goes
    // into display_payload so the UI / debug can show which slot served
    // the row; the model never sees the tag.
    let chain: Vec<(&str, &dyn VendorQuote)> = vec![
        ("primary", &*vendors.tushare),
        ("fallback-1", &*vendors.sina),
    ];
    let (quote, source_tag, source_vendor, attempts) =
        match try_chain(&chain, &symbol).await {
            Ok((q, tag, name, attempts)) => (q, tag, name, attempts),
            Err(attempts) => {
                let summary = attempts
                    .iter()
                    .map(|(v, msg)| format!("{v}: {msg}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                return ToolOutcome::error(format!(
                    "market_quote: every data source failed ({summary}). Try again \
                     in a moment, or ensure the primary data source is configured \
                     (see README's A-share section)."
                ));
            }
        };

    let model_output = format!(
        "{}（{}）现价 ¥{:.2}（{:+.2}, {:+.2}%）；今日开盘 ¥{:.2}，最高 ¥{:.2}，最低 ¥{:.2}，\
         成交量 {:.0} 股，成交额 ¥{:.0}。as-of: {}（{}）。",
        quote.name.as_deref().unwrap_or("(name?)"),
        quote.symbol,
        quote.price,
        quote.change,
        quote.change_pct,
        quote.open.unwrap_or(0.0),
        quote.high.unwrap_or(0.0),
        quote.low.unwrap_or(0.0),
        quote.volume.unwrap_or(0.0),
        quote.turnover.unwrap_or(0.0),
        quote.timestamp,
        quote.data_freshness,
    );

    let display_payload = serde_json::json!({
        "kind": "market_quote",
        "symbol": quote.symbol,
        "name": quote.name,
        "price": quote.price,
        "change": quote.change,
        "change_pct": quote.change_pct,
        "open": quote.open,
        "high": quote.high,
        "low": quote.low,
        "prev_close": quote.prev_close,
        "volume": quote.volume,
        "turnover": quote.turnover,
        "timestamp": quote.timestamp,
        "data_freshness": quote.data_freshness,
        "data_source": source_tag,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": source_vendor,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

/// Walk the chain. `Ok` returns the first successful (quote, source_tag,
/// vendor_name, attempt log). `Err` returns the attempt log so the
/// caller can show what every vendor said.
async fn try_chain<'a>(
    chain: &[(&'a str, &dyn VendorQuote)],
    symbol: &Symbol,
) -> Result<
    (
        crate::vendors::MarketQuote,
        &'a str,
        &'static str,
        Vec<(&'static str, String)>,
    ),
    Vec<(&'static str, String)>,
> {
    let mut attempts = Vec::new();
    for (tag, vendor) in chain {
        match vendor.fetch_quote(symbol).await {
            Ok(q) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                return Ok((q, *tag, vendor.vendor_name(), attempts));
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "market_quote vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    return Err(attempts);
                }
            }
        }
    }
    Err(attempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "market_quote");
        // The whole description must read as vendor-neutral — see
        // tests/m3_vendor_neutrality.rs for the canonical check; this
        // is a per-tool sanity that catches typos at unit-test time.
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "sina finance", "eastmoney"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
        assert_eq!(s.parameters["required"], serde_json::json!(["symbol"]));
    }

    #[test]
    fn missing_symbol_arg_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
        assert!(out.model_output.contains("symbol"));
    }

    #[test]
    fn malformed_symbol_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "AAPL" }),
        ));
        assert!(out.is_error);
    }
}
