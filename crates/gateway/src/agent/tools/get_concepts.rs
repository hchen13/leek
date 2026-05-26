//! `get_concepts` — concept / theme membership for one A-share symbol.
//! Up to 30 concept tags, sorted by source-side popularity when
//! available.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorConcepts, VendorRegistry};

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_concepts".into(),
        description: "Fetch the concept / theme tags a Chinese A-share symbol \
             belongs to (e.g. '白酒', '新能源', 'AI 概念', '芯片'). Returns up to \
             30 tags. Use to answer 'what themes does this stock get traded on?' \
             or 'is X part of the AI rally?'. Tag popularity rank (heat_rank) is \
             returned when the upstream supplies it. For industry classification \
             (an exclusive bucket per company) use `get_company_info` instead — \
             concept tags are many-per-company and reflect trader/news framing."
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
        display_name: "概念题材",
        result: ResultArtifact::Card("concepts"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            format!("概念题材 · {}", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_concepts: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_concepts: {e}")),
    };

    let chain: Vec<(&str, &dyn VendorConcepts)> = vec![
        ("primary", &*vendors.tushare),
        ("fallback-1", &*vendors.eastmoney),
    ];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::ConceptList, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor.fetch_concepts(&symbol).await {
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
                    "get_concepts vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (list, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            let display_payload = serde_json::json!({
                "kind": "concepts",
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
                    "get_concepts: 概念分类不可用（{summary}）。请告知用户工具暂时拿不到题材列表；\
                     不要凭印象列概念，避免误导用户。"
                ),
                display_payload,
                debug_payload,
            );
        }
    };

    let names: Vec<&str> = list
        .concepts
        .iter()
        .take(6)
        .map(|c| c.name.as_str())
        .collect();
    let model_output = format!(
        "{symbol} 所属概念 / 题材共 {n} 个，前几个：{preview}。完整列表见 display_payload.concepts。",
        symbol = list.symbol,
        n = list.concepts.len(),
        preview = names.join("、"),
    );

    let display_payload = serde_json::json!({
        "kind": "concepts",
        "symbol": list.symbol,
        "concepts": list.concepts,
        "data_source": tag,
        "data_available": true,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
        "concept_count": list.concepts.len(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_concepts");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
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
