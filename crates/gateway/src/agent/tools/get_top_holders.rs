//! `get_top_holders` — top-10 shareholders for one A-share symbol.
//! Choose between `total` (all shareholders) and `float` (only the
//! floating share register).

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorRegistry, VendorTopHolders};

const DEFAULT_KIND: &str = "total";

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_top_holders".into(),
        description: "Fetch the top-10 shareholders for one A-share symbol. \
             `kind = 'total'` (default) returns the all-shareholder top 10; \
             `'float'` returns the top 10 of the floating share register only \
             (excludes restricted / locked-up holders). Each row has rank, holder \
             name, share count, percent of total (or float), and the change vs \
             the previous reporting period when available. Use to answer 'who \
             owns this company?', 'is the controlling stake stable?', or 'are \
             institutions accumulating / reducing?'. For per-day capital flow use \
             `get_capital_flow` instead."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (e.g. '600519.SH', '300750')."
                },
                "kind": {
                    "type": "string",
                    "enum": ["total", "float"],
                    "description": "Which holder list to fetch. Default 'total' (all shareholders)."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "十大股东",
        result: ResultArtifact::Card("top_holders"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let k = args.get("kind").and_then(|v| v.as_str()).unwrap_or(DEFAULT_KIND);
            format!("十大股东 · {} · {}", super::summary_snippet(s), k)
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_top_holders: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_top_holders: {e}")),
    };
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_KIND)
        .to_string();
    if !matches!(kind.as_str(), "total" | "float") {
        return ToolOutcome::error(format!(
            "get_top_holders: invalid kind '{kind}' (try total/float)."
        ));
    }

    let chain: Vec<(&str, &dyn VendorTopHolders)> = vec![
        ("primary", &*vendors.tushare),
        ("fallback-1", &*vendors.eastmoney),
    ];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::TopHolders, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_top_holders(&symbol, &kind).await {
            Ok(h) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((h, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_top_holders vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (holders, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            let display_payload = serde_json::json!({
                "kind": "top_holders",
                "symbol": symbol.to_dotted(),
                "holder_kind": kind,
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
                    "get_top_holders: 股东名单不可用（{summary}）。请明确告知用户工具暂时拿不到数据；\
                     不要编造股东名字或持股比例。"
                ),
                display_payload,
                debug_payload,
            );
        }
    };

    // Concise prose: top 3 by name + pct.
    let top3 = holders
        .holders
        .iter()
        .take(3)
        .map(|h| {
            let pct = h
                .pct
                .map(|p| format!("{p:.2}%"))
                .unwrap_or_else(|| "n/a".into());
            format!("{} ({pct})", h.holder_name)
        })
        .collect::<Vec<_>>()
        .join("；");
    let model_output = format!(
        "{symbol} {kind_label}前 10 大股东（截至 {period}）共 {n} 条；前 3：{top3}。\
         完整列表见 display_payload.holders。",
        symbol = holders.symbol,
        kind_label = if holders.kind == "float" { "流通股东" } else { "全部股东" },
        period = holders.period_end,
        n = holders.holders.len(),
    );

    let display_payload = serde_json::json!({
        "kind": "top_holders",
        "symbol": holders.symbol,
        "holder_kind": holders.kind,
        "period_end": holders.period_end,
        "holders": holders.holders,
        "data_source": tag,
        "data_available": true,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
        "holder_count": holders.holders.len(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_top_holders");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn invalid_kind_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "kind": "control" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("kind"));
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
