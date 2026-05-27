//! `market_pulse(symbols)` — realtime facts batch for 1-10 symbols.
//!
//! Each symbol triple-call:
//!
//! - realtime quote (push2 batch)
//! - capital flow snapshot (same push2 batch endpoint)
//! - latest technical indicator row + 20-day MA snapshot
//!   (tushare `daily` + `stk_factor`)
//!
//! The output is a single distilled markdown table with the raw numbers
//! per symbol. No "overbought" / "buy" judgement — the agent decides.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorRegistry};

const MAX_SYMBOLS: usize = 10;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "market_pulse".into(),
        description: "Live snapshot for 1-10 A-share symbols at once. \
             Per symbol returns: real-time spot (price / 涨跌% / 成交额 \
             / 换手率), capital flow (主力 / 超大 / 大 / 中 / 小 单 net), \
             and the latest tech-indicator row (MA5/MA20 + RSI / MACD / \
             KDJ raw values).\n\
             \n\
             Inputs:\n\
             - symbols (required, array, 1-10) — A-share symbols.\n\
             \n\
             Examples: 'A 股的茅台 + 五粮液 + 泸州老窖 现在多少' → \
             symbols=['600519.SH', '000858.SZ', '000568.SZ'].\n\
             \n\
             Limits: cap at 10 symbols / call (push2 batch limit). \
             Outside trading hours the spot price + flow may be empty — \
             surface as `empty_dimensions: ['live']`. All technical \
             numbers are raw — the agent decides what 'overbought' / \
             'breakout' mean.\n\
             \n\
             Boundaries: this is the multi-symbol pulse panel. Use \
             `stock_overview` for a deep dossier, `chart_data` for the \
             K-line series, `market_overview` for indexes."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbols": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "maxItems": MAX_SYMBOLS,
                    "description": "A-share symbols (1-10)."
                }
            },
            "required": ["symbols"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "实时多股",
        result: ResultArtifact::Card("market_pulse"),
        summary: |args| {
            let n = args
                .get("symbols")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("实时 · {n} 只")
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(arr) = args.get("symbols").and_then(|v| v.as_array()) else {
        return ToolOutcome::error("market_pulse: missing 'symbols' (array).");
    };
    if arr.is_empty() {
        return ToolOutcome::error("market_pulse: 'symbols' must have ≥ 1 element.");
    }
    if arr.len() > MAX_SYMBOLS {
        return ToolOutcome::error(format!(
            "market_pulse: too many symbols ({}) — cap is {MAX_SYMBOLS}",
            arr.len()
        ));
    }
    let mut symbols: Vec<Symbol> = Vec::new();
    for v in arr {
        let raw = v.as_str().unwrap_or("");
        match Symbol::parse(raw) {
            Ok(s) => symbols.push(s),
            Err(e) => {
                return ToolOutcome::error(format!("market_pulse: '{raw}' is invalid — {e}"))
            }
        }
    }
    let t = &vendors.tushare;
    let e = &vendors.eastmoney;
    let today = chrono::Utc::now().naive_utc().date();
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    // One batch call for live quote + flow.
    let live_batch = match e.push2_batch_quote_flow(&symbols).await {
        Ok(rows) if !rows.is_empty() => {
            sources.push(format!("EastMoney push2 batch @ {today}"));
            rows
        }
        _ => {
            empty_dimensions.push("live".into());
            Vec::new()
        }
    };
    let live_map: std::collections::BTreeMap<String, _> = live_batch
        .into_iter()
        .map(|(q, f)| (q.symbol.clone(), (q, f)))
        .collect();

    // Per-symbol fan-out: 20-day daily + tech indicator latest.
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut md = String::new();
    md.push_str("## 实时多股快照\n\n");
    md.push_str(
        "| symbol | name | 现价 | 涨跌% | 成交额(亿) | 主力净(亿) | 主力净占% | MA5 | MA20 | RSI6 |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for sym in &symbols {
        let (daily_res, factor_res) = tokio::join!(t.daily(sym, 20), t.stk_factor(sym, 5));
        let candles = daily_res.unwrap_or_default();
        let factors = factor_res.unwrap_or_default();
        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let ma = |n: usize| -> Option<f64> {
            if closes.len() < n {
                return None;
            }
            Some(closes[closes.len() - n..].iter().sum::<f64>() / n as f64)
        };
        let latest_factor = factors.last().cloned();
        let live = live_map.get(&sym.to_dotted());
        let q = live.map(|(q, _)| q);
        let f = live.map(|(_, f)| f);
        let name = q
            .and_then(|x| x.name.clone())
            .unwrap_or_else(|| "-".into());
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            sym.to_dotted(),
            name,
            q.and_then(|x| x.price).map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into()),
            q.and_then(|x| x.change_pct).map(|n| format!("{n:+.2}%")).unwrap_or_else(|| "-".into()),
            q.and_then(|x| x.turnover_yuan).map(|n| format!("{:.2}", n / 1.0e8)).unwrap_or_else(|| "-".into()),
            f.and_then(|x| x.main_net_yuan).map(|n| format!("{:.2}", n / 1.0e8)).unwrap_or_else(|| "-".into()),
            f.and_then(|x| x.main_net_pct).map(|n| format!("{n:+.2}%")).unwrap_or_else(|| "-".into()),
            ma(5).map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into()),
            ma(20).map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into()),
            latest_factor.as_ref().and_then(|f| f.rsi_6).map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into()),
        ));
        rows.push(serde_json::json!({
            "symbol": sym.to_dotted(),
            "name": name,
            "quote": q,
            "flow": f,
            "ma5": ma(5),
            "ma20": ma(20),
            "tech_latest": latest_factor,
        }));
    }
    sources.push(format!("Tushare daily / stk_factor @ {today}"));

    if !empty_dimensions.is_empty() {
        md.push_str(&format!("\n_缺失维度: {}_\n", empty_dimensions.join(", ")));
    }
    if !sources.is_empty() {
        md.push_str(&format!("\n数据来源: {}\n", sources.join("; ")));
    }
    md.push_str("\n_技术指标值原样,agent 自行判断'超买/超卖'。_\n");

    let display = serde_json::json!({
        "kind": "market_pulse",
        "symbols": symbols.iter().map(|s| s.to_dotted()).collect::<Vec<_>>(),
        "rows": rows,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({ "count": symbols.len() });
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
    fn missing_symbols_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
    }

    #[test]
    fn too_many_symbols_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let arr: Vec<_> = (0..11).map(|_| "600519.SH").collect();
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbols": arr }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("cap"));
    }
}
