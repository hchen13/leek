//! `get_consensus` — sell-side analyst forecast aggregates + rating
//! distribution. Both vendors back onto paid / scraped boards that are
//! sometimes unavailable; the tool surfaces `data_available: false`
//! rather than fabricating numbers when both fail.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorConsensus, VendorRegistry};

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_consensus".into(),
        description: "Fetch sell-side analyst consensus for one A-share symbol: \
             forecast aggregates (revenue, net profit, EPS — mean / high / low) \
             for the next 1-2 fiscal years, plus a rating-distribution mix (buy / \
             overweight / hold / underweight / sell counts from recent broker \
             reports). Use when the user asks about analyst expectations, target \
             rating, or whether the market 'expects' something. Numbers are in \
             yuan (元). When upstream sources are unavailable the tool returns \
             `data_available: false` — do NOT make up consensus figures."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (e.g. '600519.SH', '300750')."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "一致预期",
        result: ResultArtifact::Card("consensus"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            format!("一致预期 · {}", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_consensus: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_consensus: {e}")),
    };

    let chain: Vec<(&str, &dyn VendorConsensus)> = vec![
        ("primary", &*vendors.tushare),
        ("fallback-1", &*vendors.eastmoney),
    ];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::AnalystConsensus, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_consensus(&symbol).await {
            Ok(c) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((c, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_consensus vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (consensus, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            let display_payload = serde_json::json!({
                "kind": "consensus",
                "symbol": symbol.to_dotted(),
                "data_available": false,
                "reason": summary.clone(),
            });
            let debug_payload = serde_json::json!({
                "symbol_input": raw,
                "symbol_normalized": symbol.to_dotted(),
                "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
            });
            return ToolOutcome::ok(
                format!(
                    "get_consensus: 一致预期数据不可用（{summary}）。请明确告知用户卖方一致预期暂时拿不到；\
                     不要编造目标价或评级数字。"
                ),
                display_payload,
                debug_payload,
            );
        }
    };

    let mix = &consensus.rating_mix;
    let total_ratings = mix.buy + mix.overweight + mix.hold + mix.underweight + mix.sell;
    let earliest = consensus.forecasts.first();
    let np_blurb = earliest
        .and_then(|f| f.net_profit_mean_yuan.map(|v| (f.year.clone(), v)))
        .map(|(y, v)| format!("{y} 年净利预期均值 ¥{:.2} 亿元", v / 1.0e8))
        .unwrap_or_else(|| "无年度净利预期".into());
    let model_output = format!(
        "{symbol} 卖方一致预期：{np}；评级覆盖 {n} 份报告，\
         买入 {buy} / 增持 {ow} / 中性 {hold} / 减持 {uw} / 卖出 {sell}。\
         完整年度预期表见 display_payload.forecasts。",
        symbol = consensus.symbol,
        np = np_blurb,
        n = total_ratings,
        buy = mix.buy,
        ow = mix.overweight,
        hold = mix.hold,
        uw = mix.underweight,
        sell = mix.sell,
    );

    let display_payload = serde_json::json!({
        "kind": "consensus",
        "symbol": consensus.symbol,
        "forecasts": consensus.forecasts,
        "rating_mix": consensus.rating_mix,
        "report_count": consensus.report_count,
        "data_source": tag,
        "data_available": true,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_consensus");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn missing_symbol_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
        assert!(out.model_output.contains("symbol"));
    }

    #[tokio::test]
    async fn vendor_failure_returns_data_available_false() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH" }),
        )
        .await;
        assert!(!out.is_error);
        assert!(out.display_payload["data_available"].is_boolean());
    }
}
