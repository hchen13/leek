//! `get_announcements` — recent corporate announcements for one A-share
//! symbol, optionally filtered by category. Up to 365-day lookback.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorAnnouncements, VendorRegistry};

const DEFAULT_DAYS: usize = 30;
const MAX_DAYS: usize = 365;
/// Categories the model can pass to filter the result. These match the
/// label set assigned by the per-vendor classifier; treating them as
/// an enum keeps the schema strict.
const CATEGORIES: &[&str] = &[
    "重大事项",
    "增减持",
    "分红配股",
    "解禁",
    "回购",
    "财报",
    "高管/治理",
    "监管",
    "其它",
];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_announcements".into(),
        description: "Fetch recent corporate announcements for one A-share symbol. \
             Returns title, publication date, best-effort category tag, and a URL \
             when available. Use when the user asks about recent news, capital \
             moves, board changes, regulatory actions, dividend / repurchase / \
             share-unlock events. `days` controls the lookback window (default 30, \
             max 365). `type` optionally filters by a category tag (see enum). \
             Same-title items are de-duplicated. For deeper context on a single \
             announcement, follow up with `web_fetch` on the returned URL."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (e.g. '600519.SH', '300750')."
                },
                "days": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_DAYS,
                    "description": "Lookback window in days (max 365, default 30)."
                },
                "type": {
                    "type": "string",
                    "enum": CATEGORIES,
                    "description": "Optional category filter (substring match on \
                                    the tag). Pass nothing to get all announcements."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "公告",
        result: ResultArtifact::Card("announcements"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let d = args
                .get("days")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_DAYS as u64);
            format!("公告 · {} · {d}d", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_announcements: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_announcements: {e}")),
    };
    let days = args
        .get("days")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_DAYS);
    if days == 0 || days > MAX_DAYS {
        return ToolOutcome::error(format!(
            "get_announcements: days must be in 1..={MAX_DAYS} (got {days})."
        ));
    }
    let category = args.get("type").and_then(|v| v.as_str()).map(String::from);
    if let Some(c) = &category {
        if !CATEGORIES.contains(&c.as_str()) {
            return ToolOutcome::error(format!(
                "get_announcements: invalid type '{c}' (see schema enum for valid values)."
            ));
        }
    }

    let chain: Vec<(&str, &dyn VendorAnnouncements)> = vec![
        ("primary", &*vendors.tushare),
        ("fallback-1", &*vendors.eastmoney),
    ];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::AnnouncementList, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor
            .fetch_announcements(&symbol, days, category.as_deref())
            .await
        {
            Ok(list) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((list, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_announcements vendor attempt failed",
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
                "kind": "announcements",
                "symbol": symbol.to_dotted(),
                "days": days,
                "category_filter": category,
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
                    "get_announcements: 公告数据不可用（{summary}）。请告知用户无法取得近期公告；\
                     不要凭想象列具体公告题目。"
                ),
                display_payload,
                debug_payload,
            );
        }
    };

    // First few titles in prose so the model has something to cite
    // without reading display_payload.
    let highlights = list
        .items
        .iter()
        .take(5)
        .map(|a| format!("{} {}", a.date, a.title))
        .collect::<Vec<_>>()
        .join("；");
    let model_output = format!(
        "{symbol} 近 {days} 天{cat}共 {n} 条公告。前 5 条：{highlights}。\
         完整列表见 display_payload.items。",
        symbol = list.symbol,
        days = list.days,
        cat = match &list.category_filter {
            Some(c) => format!("（类型 = {c}）"),
            None => "".into(),
        },
        n = list.items.len(),
    );

    let display_payload = serde_json::json!({
        "kind": "announcements",
        "symbol": list.symbol,
        "days": list.days,
        "category_filter": list.category_filter,
        "items": list.items,
        "data_source": tag,
        "data_available": true,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
        "item_count": list.items.len(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_announcements");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn days_out_of_range_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "days": 999 }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("days"));
    }

    #[test]
    fn invalid_type_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "type": "宇宙合并" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("type"));
    }

    #[tokio::test]
    async fn vendor_failure_returns_data_available_false() {
        // Both primary (no token) and fallback may refuse; we only
        // require the tool returned a structured payload, not a hard
        // error. When fallback happens to succeed live, `data_available`
        // is `true`. Either is acceptable — what we're really guarding
        // against is is_error=true or a panic.
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
