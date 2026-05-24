//! `get_capital_flow` — net flow over a 1d / 5d / 20d window for one
//! A-share symbol, broken down by main-force vs retail. Northbound
//! (Stock Connect) flow is included when the upstream quota allows;
//! otherwise the response gracefully degrades to `north_flow_available:
//! false` rather than failing.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorCapitalFlow, VendorRegistry};

const DEFAULT_PERIOD: &str = "1d";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_capital_flow".into(),
        description: "Fetch capital flow for one A-share symbol over a rolling \
             window (1, 5, or 20 trading days). Returns net inflow, main-force \
             (institutional / large trade) flow, retail flow, and — when \
             available — northbound (Stock Connect) flow. When the upstream \
             northbound quota is exhausted, `north_flow_available` comes back \
             `false` with `north_flow: null` and a warning string; the rest of \
             the data is still valid. All amounts in yuan (元), signed."
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
                    "enum": ["1d", "5d", "20d"],
                    "description": "Aggregation window. Default '1d' (just the most recent day)."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "资金流",
        result: ResultArtifact::Card("capital_flow"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let p = args
                .get("period")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_PERIOD);
            format!("资金流 · {} · {}", super::summary_snippet(s), p)
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_capital_flow: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_capital_flow: {e}")),
    };
    let period = args
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_PERIOD)
        .to_string();
    if !matches!(period.as_str(), "1d" | "5d" | "20d") {
        return ToolOutcome::error(format!(
            "get_capital_flow: invalid period '{period}' (try 1d/5d/20d)."
        ));
    }

    // Only the token-backed primary source ships moneyflow in M3 —
    // public fallbacks don't expose per-stock retail/main split.
    let chain: Vec<(&str, &dyn VendorCapitalFlow)> = vec![("primary", &*vendors.tushare)];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::CapitalFlow, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_capital_flow(&symbol, &period).await {
            Ok(flow) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((flow, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_capital_flow vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (flow, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            return ToolOutcome::error(format!(
                "get_capital_flow: all data sources failed ({summary})."
            ));
        }
    };

    let fmt_yi = |v: f64| format!("{:+.2} 亿元", v / 1.0e8);
    let north = if flow.north_flow_available {
        flow.north_flow
            .map(fmt_yi)
            .unwrap_or_else(|| "n/a".into())
    } else {
        "不可用（quota 未开通或受限）".into()
    };
    let model_output = format!(
        "{} {} 资金流：总净流入 {}，主力 {}，散户 {}，北向 {}。{}",
        flow.symbol,
        flow.period,
        fmt_yi(flow.net_inflow),
        flow.main_flow.map(fmt_yi).unwrap_or_else(|| "n/a".into()),
        flow.retail_flow.map(fmt_yi).unwrap_or_else(|| "n/a".into()),
        north,
        if flow.warnings.is_empty() {
            "".into()
        } else {
            format!(" 备注：{}", flow.warnings.join("；"))
        },
    );

    let display_payload = serde_json::json!({
        "kind": "capital_flow",
        "symbol": flow.symbol,
        "period": flow.period,
        "net_inflow": flow.net_inflow,
        "main_flow": flow.main_flow,
        "retail_flow": flow.retail_flow,
        "north_flow_available": flow.north_flow_available,
        "north_flow": flow.north_flow,
        "warnings": flow.warnings,
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
        assert_eq!(s.name, "get_capital_flow");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn invalid_period_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "period": "30d" }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn missing_symbol_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
    }
}
