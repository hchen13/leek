//! `get_industry_peers` — same-industry comparison set for one A-share
//! symbol. Returns 8-12 peer rows (target first) plus a median + target
//! quantile on the dimension's principal metric.
//!
//! Dimensions:
//! - `"valuation"` (default): pe_ttm / pb / ps_ttm / dv_ttm / total_mv
//! - `"growth"`: or_yoy / netprofit_yoy
//! - `"profitability"`: roe / grossprofit_margin / netprofit_margin
//!
//! Fallback chain: primary → (no public fallback for industry peer
//! enrichment in M4.1; a future revision can plug EastMoney F10's
//! "行业对比" board).
//!
//! When the primary refuses to serve we surface a structured error
//! that includes the attempts log; the model gets a clear note that
//! peer comparison is unavailable rather than fabricated.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorIndustryPeers, VendorRegistry};

const DEFAULT_DIMENSION: &str = "valuation";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_industry_peers".into(),
        description: "Fetch a peer set (up to 12 companies) from the same A-share \
             industry, with comparison metrics on one of three dimensions: \
             'valuation' (default) — trailing P/E, P/B, P/S, dividend yield, total \
             market cap; 'growth' — revenue YoY and net-profit YoY; 'profitability' \
             — ROE, gross margin, net margin. The target symbol is always the first \
             peer; the response also returns the industry name, the principal \
             metric (e.g. 'pe_ttm' for valuation), the peer median for it, and the \
             target's quantile (0..1) within the peer set. Use to answer 'is X \
             expensive vs its industry?' or 'who else is in this space?'. For \
             absolute fundamentals on one company use `get_financials`."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (e.g. '600519.SH', '300750')."
                },
                "dimension": {
                    "type": "string",
                    "enum": ["valuation", "growth", "profitability"],
                    "description": "Which metric set to compare on. Default 'valuation'."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "行业对比",
        result: ResultArtifact::Card("industry_peers"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let d = args
                .get("dimension")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_DIMENSION);
            format!("行业对比 · {} · {}", super::summary_snippet(s), d)
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_industry_peers: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_industry_peers: {e}")),
    };
    let dimension = args
        .get("dimension")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_DIMENSION)
        .to_string();
    if !matches!(dimension.as_str(), "valuation" | "growth" | "profitability") {
        return ToolOutcome::error(format!(
            "get_industry_peers: invalid dimension '{dimension}' \
             (try valuation/growth/profitability)."
        ));
    }

    let chain: Vec<(&str, &dyn VendorIndustryPeers)> = vec![("primary", &*vendors.tushare)];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::IndustryPeers, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_industry_peers(&symbol, &dimension).await {
            Ok(peers) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((peers, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_industry_peers vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (peers, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            // Spec: when every source fails, surface a structured
            // `data_available: false` so the model never invents peers.
            let display_payload = serde_json::json!({
                "kind": "industry_peers",
                "symbol": symbol.to_dotted(),
                "dimension": dimension,
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
                    "get_industry_peers: 同行业对比数据不可用（{summary}）。请勿臆造数据；\
                     建议告知用户该工具暂时不可用，可改用 get_company_info / get_financials 看绝对值。"
                ),
                display_payload,
                debug_payload,
            );
        }
    };

    // Compact prose summary — full table goes in display_payload.
    let target_row = peers.peers.iter().find(|p| p.is_target);
    let target_principal = target_row
        .and_then(|r| r.metrics.get(&peers.principal_metric).and_then(|v| *v))
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "n/a".into());
    let median = peers
        .median
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "n/a".into());
    let quantile = peers
        .target_quantile
        .map(|v| format!("{:.0}%", v * 100.0))
        .unwrap_or_else(|| "n/a".into());
    let model_output = format!(
        "{symbol} 所在「{industry}」行业共选取 {n} 家可比公司（含目标），\
         {dim} 维度主指标 = {principal}：目标值 {tv}，行业中位 {med}，目标分位 {q}。\
         完整对比表见 display_payload.peers。",
        symbol = peers.symbol,
        industry = peers.industry,
        n = peers.peers.len(),
        dim = peers.dimension,
        principal = peers.principal_metric,
        tv = target_principal,
        med = median,
        q = quantile,
    );

    let display_payload = serde_json::json!({
        "kind": "industry_peers",
        "symbol": peers.symbol,
        "industry": peers.industry,
        "sub_industry": peers.sub_industry,
        "dimension": peers.dimension,
        "principal_metric": peers.principal_metric,
        "median": peers.median,
        "target_quantile": peers.target_quantile,
        "peers": peers.peers,
        "data_source": tag,
        "data_available": true,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
        "peer_count": peers.peers.len(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_industry_peers");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
        assert_eq!(s.parameters["required"], serde_json::json!(["symbol"]));
    }

    #[test]
    fn invalid_dimension_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "dimension": "everything" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("dimension"));
    }

    #[test]
    fn missing_symbol_arg_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
        assert!(out.model_output.contains("symbol"));
    }

    #[tokio::test]
    async fn vendor_failure_returns_data_available_false_not_error() {
        // No tushare token configured + no fallback = structured
        // graceful payload, not is_error=true (per dispatch md §A1).
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH" }),
        )
        .await;
        // Not is_error — the tool succeeded in returning a structured
        // "unavailable" answer; the model uses this to refuse to fabricate.
        assert!(!out.is_error);
        assert_eq!(out.display_payload["data_available"], false);
        assert!(out.display_payload["reason"].is_string());
    }
}
