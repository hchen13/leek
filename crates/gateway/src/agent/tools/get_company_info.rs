//! `get_company_info` — company profile + latest indicators snapshot
//! (market cap, P/E, P/B, dividend yield) for one A-share symbol.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorCompanyInfo, VendorRegistry};

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_company_info".into(),
        description: "Fetch company profile + latest valuation indicators for one \
             A-share symbol: full name, industry, listing exchange + date, plus a \
             snapshot of market cap, float market cap, trailing P/E, P/B, and \
             dividend yield. Use as orientation before financial analysis. For \
             time-series fundamentals use `get_financials`; for prices use \
             `market_quote` or `get_candlesticks`."
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
        display_name: "公司简介",
        result: ResultArtifact::Card("company_info"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            format!("公司简介 · {}", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_company_info: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_company_info: {e}")),
    };

    // Like financials: only the token-backed primary source ships in
    // M3 — structured company metadata has no real public fallback.
    let chain: Vec<(&str, &dyn VendorCompanyInfo)> = vec![("primary", &*vendors.tushare)];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::CompanyInfo, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_company_info(&symbol).await {
            Ok(info) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((info, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_company_info vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (info, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            return ToolOutcome::error(format!(
                "get_company_info: all data sources failed ({summary}). Company \
                 metadata is currently only available via the primary vendor."
            ));
        }
    };

    let mc = info
        .latest_indicators
        .get("market_cap")
        .and_then(|v| *v)
        .map(|v| format!("{:.2} 亿元", v / 1.0e8))
        .unwrap_or_else(|| "n/a".into());
    let pe = info
        .latest_indicators
        .get("pe_ttm")
        .and_then(|v| *v)
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "n/a".into());
    let pb = info
        .latest_indicators
        .get("pb")
        .and_then(|v| *v)
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "n/a".into());
    let dy = info
        .latest_indicators
        .get("dividend_yield_pct")
        .and_then(|v| *v)
        .map(|v| format!("{v:.2}%"))
        .unwrap_or_else(|| "n/a".into());
    let model_output = format!(
        "{name}（{symbol}）— {industry}，{exchange}，上市 {list}。\n\
         市值 {mc}；TTM P/E {pe}，P/B {pb}，股息率 {dy}。",
        name = info.name,
        symbol = info.symbol,
        industry = info.industry.as_deref().unwrap_or("行业未知"),
        exchange = info.exchange.as_deref().unwrap_or("交易所未知"),
        list = info.list_date.as_deref().unwrap_or("上市日未知"),
    );

    let display_payload = serde_json::json!({
        "kind": "company_info",
        "symbol": info.symbol,
        "name": info.name,
        "industry": info.industry,
        "sector": info.sector,
        "exchange": info.exchange,
        "list_date": info.list_date,
        "business_scope": info.business_scope,
        "latest_indicators": info.latest_indicators,
        "data_source": tag,
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
        assert_eq!(s.name, "get_company_info");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn malformed_symbol_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "INVALID" }),
        ));
        assert!(out.is_error);
    }
}
