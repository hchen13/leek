//! `chart_data(symbol, range?, kind?)` — raw OHLCV + indicators.
//!
//! The canvas card takes the structured payload; the `model_output`
//! stays compact (a one-paragraph numeric summary) so context isn't
//! drowned in OHLC arrays.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorRegistry};

const DEFAULT_RANGE: &str = "3m";
const RANGE_CHOICES: &[&str] = &["1d", "5d", "1m", "3m", "6m", "1y", "3y", "5y"];
const DEFAULT_KIND: &str = "candles";
const KIND_CHOICES: &[&str] = &["candles", "with_volume", "with_ma", "with_indicators"];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "chart_data".into(),
        description: "Raw OHLCV (+ optional technical indicators) for one \
             symbol. The canvas card consumes the full data; the model \
             only receives a one-paragraph numeric summary.\n\
             \n\
             Inputs:\n\
             - symbol (required).\n\
             - range (optional, default '3m'): one of 1d, 5d, 1m, 3m, \
             6m, 1y, 3y, 5y. Auto picks the bar grain (intraday for \
             1d/5d, daily for 1m-1y, weekly/monthly for 3y/5y).\n\
             - kind (optional, default 'candles'): 'candles' (OHLC only), \
             'with_volume', 'with_ma' (adds MA5/20/60 derived from the \
             closes), 'with_indicators' (adds RSI / KDJ / MACD / BOLL \
             from a separate tech-factor query).\n\
             \n\
             Examples: 'show me 600519 6m chart' → range='6m'.\n\
             \n\
             Limits: returns at most ~600 bars per call. M4.1.1 ships \
             candles + MA + indicators; intraday 1d/5d use daily bars as \
             a placeholder (intraday bars land in M4.2).\n\
             \n\
             Boundaries: pure raw-data fetch for charts. Use \
             `stock_overview` focus='technical' for distilled tech \
             commentary in markdown."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" },
                "range": {
                    "type": "string",
                    "enum": RANGE_CHOICES,
                    "description": "Time range (default '3m')."
                },
                "kind": {
                    "type": "string",
                    "enum": KIND_CHOICES,
                    "description": "Output kind (default 'candles')."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "K 线数据",
        result: ResultArtifact::Card("chart_data"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let r = args
                .get("range")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_RANGE);
            format!("K 线 · {} · {r}", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("chart_data: missing 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("chart_data: {e}")),
    };
    let range = args
        .get("range")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_RANGE)
        .to_string();
    if !RANGE_CHOICES.contains(&range.as_str()) {
        return ToolOutcome::error(format!(
            "chart_data: invalid range '{range}' (try {})",
            RANGE_CHOICES.join("/")
        ));
    }
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_KIND)
        .to_string();
    if !KIND_CHOICES.contains(&kind.as_str()) {
        return ToolOutcome::error(format!(
            "chart_data: invalid kind '{kind}' (try {})",
            KIND_CHOICES.join("/")
        ));
    }
    let t = &vendors.tushare;
    let today = chrono::Utc::now().naive_utc().date();
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    let (granularity, want_count) = match range.as_str() {
        "1d" => ("daily", 1),
        "5d" => ("daily", 5),
        "1m" => ("daily", 22),
        "3m" => ("daily", 66),
        "6m" => ("daily", 132),
        "1y" => ("daily", 252),
        "3y" => ("weekly", 156),
        "5y" => ("monthly", 60),
        _ => ("daily", 60),
    };

    let candles = match granularity {
        "daily" => t.daily(&symbol, want_count).await,
        "weekly" => t.period_candles(&symbol, "weekly", want_count).await,
        "monthly" => t.period_candles(&symbol, "monthly", want_count).await,
        _ => Err(crate::vendors::VendorError::fatal(
            "tushare",
            "internal: unknown granularity",
        )),
    };
    let candles = match candles {
        Ok(c) if !c.is_empty() => {
            sources.push(format!("Tushare {granularity} @ {today}"));
            c
        }
        Ok(_) | Err(_) => {
            empty_dimensions.push("candles".into());
            Vec::new()
        }
    };
    let factors = if kind == "with_indicators" {
        match t.stk_factor(&symbol, want_count).await {
            Ok(f) if !f.is_empty() => {
                sources.push(format!("Tushare stk_factor @ {today}"));
                f
            }
            _ => {
                empty_dimensions.push("stk_factor".into());
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let mut md = format!(
        "## {} {} 区间 K 线数据\n\n",
        symbol.to_dotted(),
        range,
    );
    if !candles.is_empty() {
        let first = candles.first().unwrap();
        let last = candles.last().unwrap();
        let high = candles.iter().map(|c| c.high).fold(f64::MIN, f64::max);
        let low = candles.iter().map(|c| c.low).fold(f64::MAX, f64::min);
        let vol_avg = candles.iter().map(|c| c.volume).sum::<f64>() / candles.len() as f64;
        let pct = if first.close > 0.0 {
            (last.close - first.close) / first.close * 100.0
        } else {
            0.0
        };
        md.push_str(&format!(
            "- 起 {} ¥{:.2} → 终 {} ¥{:.2} ({:+.2}%)\n- 区间最高 ¥{:.2} / 最低 ¥{:.2}\n- 平均日成交量 {:.0} 股\n- bar 数 {}\n",
            first.date, first.close, last.date, last.close, pct, high, low, vol_avg, candles.len(),
        ));
    } else {
        md.push_str("无 K 线数据。\n");
    }

    let mut ma_payload = serde_json::json!({});
    if kind == "with_ma" || kind == "with_indicators" {
        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let ma = |n: usize| -> Vec<Option<f64>> {
            (0..closes.len())
                .map(|i| {
                    if i + 1 < n {
                        None
                    } else {
                        Some(closes[i + 1 - n..=i].iter().sum::<f64>() / n as f64)
                    }
                })
                .collect()
        };
        ma_payload = serde_json::json!({
            "ma5": ma(5),
            "ma20": ma(20),
            "ma60": ma(60),
        });
    }

    let display = serde_json::json!({
        "kind": "chart_data",
        "symbol": symbol.to_dotted(),
        "range": range,
        "view": kind,
        "candles": candles,
        "indicators": factors,
        "moving_averages": ma_payload,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "range": range,
        "view": kind,
    });
    ToolOutcome::ok(md, display, debug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_vendor_neutral() {
        let s = spec();
        let d = s.description.to_lowercase();
        for needle in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(needle), "leaked '{needle}'");
        }
        for needle in ["新浪", "东方财富"] {
            assert!(!s.description.contains(needle));
        }
    }

    #[test]
    fn invalid_range_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "range": "decade" }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn invalid_kind_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "kind": "vibes" }),
        ));
        assert!(out.is_error);
    }
}
