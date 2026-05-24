//! `get_candlesticks` — historical OHLCV bars for one A-share symbol.
//!
//! Default: `period=1d`, `count=120` (about half a year of daily bars).
//! Hard cap: `count ≤ 500` — beyond that the model should narrow the
//! window or pick a coarser period. Fallback chain: primary → fallback.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorCandle, VendorRegistry};

const DEFAULT_PERIOD: &str = "1d";
const DEFAULT_COUNT: usize = 120;
const MAX_COUNT: usize = 500;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_candlesticks".into(),
        description: "Fetch historical OHLCV bars (candlesticks) for one A-share \
             symbol. Returns up to 500 bars in chronological order (oldest first). \
             Use for trend / technical analysis. For a single snapshot price, \
             prefer `market_quote`. Symbol accepts '600519.SH', '600519', or \
             'sh600519'. Daily bars are sufficient for most reasoning; switch to \
             weekly/monthly only for very long-horizon studies."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (e.g. '600519.SH', '300750')."
                },
                "period": {
                    "type": "string",
                    "enum": ["1d", "1w", "1mo"],
                    "description": "Bar period. Defaults to '1d' (daily)."
                },
                "count": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_COUNT,
                    "description": "Number of bars to fetch (max 500, default 120)."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "K 线历史",
        result: ResultArtifact::Card("candlesticks"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let period = args
                .get("period")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_PERIOD);
            format!("K 线 · {} {}", super::summary_snippet(s), period)
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_candlesticks: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_candlesticks: {e}")),
    };
    let period = args
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_PERIOD)
        .to_string();
    if !matches!(period.as_str(), "1d" | "1w" | "1mo") {
        return ToolOutcome::error(format!(
            "get_candlesticks: invalid period '{period}' (try 1d/1w/1mo)."
        ));
    }
    let count = args
        .get("count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_COUNT);
    if count == 0 {
        return ToolOutcome::error("get_candlesticks: count must be ≥ 1.");
    }
    if count > MAX_COUNT {
        return ToolOutcome::error(format!(
            "get_candlesticks: count={count} exceeds the cap of {MAX_COUNT}. \
             Please narrow the window or switch period."
        ));
    }

    // Fallback chain: primary token-backed source → public token-free
    // source. The tag goes into display_payload; the model never sees it.
    let chain: Vec<(&str, &dyn VendorCandle)> = vec![
        ("primary", &*vendors.tushare),
        ("fallback-1", &*vendors.eastmoney),
    ];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(Vec<crate::vendors::Candle>, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_candles(&symbol, &period, count).await {
            Ok(candles) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((candles, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_candlesticks vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (candles, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            return ToolOutcome::error(format!(
                "get_candlesticks: every data source failed ({summary})."
            ));
        }
    };

    // Compact model summary — full bars in payload, not in prose. The
    // model can ask for specifics by reading display_payload.candles.
    let last = candles.last().unwrap();
    let first = candles.first().unwrap();
    let highest = candles
        .iter()
        .map(|c| c.high)
        .fold(f64::MIN, f64::max);
    let lowest = candles
        .iter()
        .map(|c| c.low)
        .fold(f64::MAX, f64::min);
    let model_output = format!(
        "{symbol} {period} 线返回 {n} 根 K 线（{first_date} → {last_date}）。\
         区间内最高 ¥{high:.2}，最低 ¥{low:.2}；末根收盘 ¥{close:.2}。\
         完整 OHLCV 数据见 display_payload.candles。",
        symbol = symbol.to_dotted(),
        period = period,
        n = candles.len(),
        first_date = first.date,
        last_date = last.date,
        high = highest,
        low = lowest,
        close = last.close,
    );

    let display_payload = serde_json::json!({
        "kind": "candlesticks",
        "symbol": symbol.to_dotted(),
        "period": period,
        "count": candles.len(),
        "candles": candles,
        "data_source": tag,
        "summary": {
            "first_date": first.date,
            "last_date": last.date,
            "highest": highest,
            "lowest": lowest,
            "last_close": last.close,
        },
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
        "requested_count": count,
        "returned_count": candles.len(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_candlesticks");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn count_over_cap_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "count": 1000 }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("cap"));
    }

    #[test]
    fn bad_period_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "period": "5min" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("period"));
    }
}
