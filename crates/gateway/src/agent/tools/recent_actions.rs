//! `recent_actions` — single-stock event stream over a window.
//!
//! Multiple optional event types fan out concurrently and merge into a
//! reverse-chronological timeline of `{date, type, title, abstract,
//! source url}`.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorRegistry};

const DEFAULT_DAYS: usize = 30;
const MAX_DAYS: usize = 365;
const ALL_FILTERS: &[&str] = &[
    "announcement",
    "insider_trade",
    "dividend",
    "share_unlock",
    "block_trade",
    "top_list",
    "pledge",
    "repurchase",
    "institution_visit",
];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "recent_actions".into(),
        description: "Single-stock event stream over a window. Merges \
             announcements + 增减持 + 分红 + 限售解禁 + 大宗交易 + 龙虎榜 \
             + 股权质押 + 公司回购 + 机构调研 into a reverse-chronological \
             timeline, each row a `{date, type chip, title, abstract, \
             source url}` line.\n\
             \n\
             Inputs:\n\
             - symbol (required) — A-share symbol.\n\
             - days (optional, 1-365, default 30) — how far back.\n\
             - filter (optional) — array, any of: announcement, \
             insider_trade, dividend, share_unlock, block_trade, top_list, \
             pledge, repurchase, institution_visit. Empty = all.\n\
             \n\
             Examples: '茅台最近 60 天有什么事' → days=60, filter=[]; \
             '宁德最近的大宗交易和龙虎榜' → \
             filter=['block_trade','top_list'].\n\
             \n\
             Limits: returns ≤ 80 events total. Per-section vendor \
             outages surface as `empty_dimensions: ['dividend', \
             'block_trade', …]`. Do NOT retry — those types are empty \
             for this symbol/window.\n\
             \n\
             Boundaries: this is the event timeline. Use `stock_overview` \
             for company / financial snapshots, `research_sentiment` for \
             sell-side coverage."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" },
                "days": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_DAYS,
                    "description": "Lookback window (default 30, max 365)."
                },
                "filter": {
                    "type": "array",
                    "items": { "type": "string", "enum": ALL_FILTERS },
                    "description": "Event type filter (empty = all)."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "近期事件流",
        result: ResultArtifact::Card("recent_actions"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(30);
            format!("事件 · {} · {days}天", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw_sym) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("recent_actions: missing required 'symbol'.");
    };
    let symbol = match Symbol::parse(raw_sym) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("recent_actions: {e}")),
    };
    let days = args
        .get("days")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_DAYS);
    if days == 0 || days > MAX_DAYS {
        return ToolOutcome::error(format!(
            "recent_actions: days must be in 1..={MAX_DAYS} (got {days})"
        ));
    }
    let filters: Vec<String> = args
        .get("filter")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    for f in &filters {
        if !ALL_FILTERS.contains(&f.as_str()) {
            return ToolOutcome::error(format!(
                "recent_actions: invalid filter '{f}' (try {})",
                ALL_FILTERS.join("/")
            ));
        }
    }
    let want = |s: &str| filters.is_empty() || filters.iter().any(|f| f == s);

    let t = &vendors.tushare;
    let e = &vendors.eastmoney;
    let today = chrono::Utc::now().naive_utc().date();
    let start = today - chrono::Duration::days(days as i64);
    let start_compact = start.format("%Y%m%d").to_string();
    let end_compact = today.format("%Y%m%d").to_string();
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    // Fan-out everything that's requested.
    let (ann_t, ann_e, insider, dividend, unlock, block, top, pledge, repurchase, visits) = tokio::join!(
        async {
            if want("announcement") {
                Some(t.anns_d(&symbol, &start_compact, &end_compact).await)
            } else {
                None
            }
        },
        async {
            if want("announcement") {
                Some(e.announcements(&symbol, days).await)
            } else {
                None
            }
        },
        async {
            if want("insider_trade") {
                Some(t.stk_holdertrade(&symbol, 50).await)
            } else {
                None
            }
        },
        async {
            if want("dividend") {
                Some(t.dividend(&symbol, 30).await)
            } else {
                None
            }
        },
        async {
            if want("share_unlock") {
                Some(t.share_float(&symbol, 30).await)
            } else {
                None
            }
        },
        async {
            if want("block_trade") {
                Some(t.block_trade(&symbol, 30).await)
            } else {
                None
            }
        },
        async {
            if want("top_list") {
                Some(t.top_list_symbol(&symbol, 30).await)
            } else {
                None
            }
        },
        async {
            if want("pledge") {
                Some(t.pledge_stat(&symbol, 12).await)
            } else {
                None
            }
        },
        async {
            if want("repurchase") {
                Some(t.repurchase(&symbol, 20).await)
            } else {
                None
            }
        },
        async {
            if want("institution_visit") {
                Some(t.stk_surv_window(Some(&symbol), &start_compact, &end_compact).await)
            } else {
                None
            }
        },
    );

    #[derive(Debug, serde::Serialize)]
    struct Event {
        date: String,
        kind: String,
        title: String,
        abstract_text: Option<String>,
        url: Option<String>,
    }
    let mut events: Vec<Event> = Vec::new();
    let in_window = |d: &str| {
        chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map(|nd| nd >= start && nd <= today)
            .unwrap_or(true)
    };
    // Announcements — merge both sources, dedupe by title.
    if want("announcement") {
        let mut seen = std::collections::HashSet::new();
        let mut count = 0;
        if let Some(Ok(rows)) = &ann_t {
            sources.push("Tushare anns_d".into());
            for a in rows {
                if !in_window(&a.date) {
                    continue;
                }
                if !seen.insert(a.title.clone()) {
                    continue;
                }
                count += 1;
                events.push(Event {
                    date: a.date.clone(),
                    kind: "announcement".into(),
                    title: a.title.clone(),
                    abstract_text: a.category.clone(),
                    url: a.url.clone(),
                });
            }
        }
        if let Some(Ok(rows)) = &ann_e {
            sources.push("EastMoney np-anotice-stock".into());
            for a in rows {
                if !in_window(&a.date) {
                    continue;
                }
                if !seen.insert(a.title.clone()) {
                    continue;
                }
                count += 1;
                events.push(Event {
                    date: a.date.clone(),
                    kind: "announcement".into(),
                    title: a.title.clone(),
                    abstract_text: a.category.clone(),
                    url: a.pdf_url.clone().or_else(|| a.url.clone()),
                });
            }
        }
        if count == 0 {
            empty_dimensions.push("announcement".into());
        }
    }
    if want("insider_trade") {
        if let Some(Ok(rows)) = &insider {
            sources.push("Tushare stk_holdertrade".into());
            let pre = events.len();
            for r in rows {
                if !in_window(&r.ann_date) {
                    continue;
                }
                events.push(Event {
                    date: r.ann_date.clone(),
                    kind: "insider_trade".into(),
                    title: format!(
                        "{} {}",
                        r.holder_name,
                        r.direction.clone().unwrap_or_default()
                    ),
                    abstract_text: Some(format!(
                        "变动 {} 股,变动比例 {} %,变动后持有 {} 股 ({} %)",
                        fmt_num(r.change_vol_shares),
                        fmt_num(r.change_ratio_pct),
                        fmt_num(r.after_shares),
                        fmt_num(r.after_ratio_pct),
                    )),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("insider_trade".into());
            }
        } else {
            empty_dimensions.push("insider_trade".into());
        }
    }
    if want("dividend") {
        if let Some(Ok(rows)) = &dividend {
            sources.push("Tushare dividend".into());
            let pre = events.len();
            for r in rows {
                if !in_window(&r.ann_date) {
                    continue;
                }
                events.push(Event {
                    date: r.ann_date.clone(),
                    kind: "dividend".into(),
                    title: format!(
                        "{} 分红({} 元 / 10 股送转)",
                        r.process.clone().unwrap_or_default(),
                        fmt_num(r.cash_div_pretax),
                    ),
                    abstract_text: Some(format!(
                        "送股 {} / 报告期 {} / 除权日 {}",
                        fmt_num(r.stk_div),
                        r.end_date,
                        r.ex_date.clone().unwrap_or_default(),
                    )),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("dividend".into());
            }
        } else {
            empty_dimensions.push("dividend".into());
        }
    }
    if want("share_unlock") {
        if let Some(Ok(rows)) = &unlock {
            sources.push("Tushare share_float".into());
            let pre = events.len();
            for r in rows {
                if !in_window(&r.ann_date) {
                    continue;
                }
                events.push(Event {
                    date: r.ann_date.clone(),
                    kind: "share_unlock".into(),
                    title: format!(
                        "限售解禁 {} 股({} %)将于 {} 上市流通",
                        fmt_num(r.float_share),
                        fmt_num(r.float_ratio),
                        r.float_date,
                    ),
                    abstract_text: r.holder_name.clone(),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("share_unlock".into());
            }
        } else {
            empty_dimensions.push("share_unlock".into());
        }
    }
    if want("block_trade") {
        if let Some(Ok(rows)) = &block {
            sources.push("Tushare block_trade".into());
            let pre = events.len();
            for r in rows {
                if !in_window(&r.trade_date) {
                    continue;
                }
                events.push(Event {
                    date: r.trade_date.clone(),
                    kind: "block_trade".into(),
                    title: format!(
                        "大宗交易 价 {} / 量 {} 万股 / 额 {} 万元",
                        fmt_num(r.price),
                        fmt_num(r.vol_wan_shares),
                        fmt_num(r.amount_wan_yuan),
                    ),
                    abstract_text: Some(format!(
                        "买方 {} / 卖方 {}",
                        r.buyer.clone().unwrap_or_else(|| "-".into()),
                        r.seller.clone().unwrap_or_else(|| "-".into()),
                    )),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("block_trade".into());
            }
        } else {
            empty_dimensions.push("block_trade".into());
        }
    }
    if want("top_list") {
        if let Some(Ok(rows)) = &top {
            sources.push("Tushare top_list".into());
            let pre = events.len();
            for r in rows {
                if !in_window(&r.trade_date) {
                    continue;
                }
                events.push(Event {
                    date: r.trade_date.clone(),
                    kind: "top_list".into(),
                    title: format!(
                        "龙虎榜 收 {} ({}%) 净额 {} 换手 {}",
                        fmt_num(r.close),
                        fmt_num(r.pct_change),
                        fmt_num(r.net_amount),
                        fmt_num(r.turnover_rate),
                    ),
                    abstract_text: r.reason.clone(),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("top_list".into());
            }
        } else {
            empty_dimensions.push("top_list".into());
        }
    }
    if want("pledge") {
        if let Some(Ok(rows)) = &pledge {
            sources.push("Tushare pledge_stat".into());
            let pre = events.len();
            for r in rows {
                events.push(Event {
                    date: r.end_date.clone(),
                    kind: "pledge".into(),
                    title: format!(
                        "质押 {} 笔 / 比 {}%",
                        fmt_num(r.pledge_count),
                        fmt_num(r.pledge_ratio_pct),
                    ),
                    abstract_text: None,
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("pledge".into());
            }
        } else {
            empty_dimensions.push("pledge".into());
        }
    }
    if want("repurchase") {
        if let Some(Ok(rows)) = &repurchase {
            sources.push("Tushare repurchase".into());
            let pre = events.len();
            for r in rows {
                if !in_window(&r.ann_date) {
                    continue;
                }
                events.push(Event {
                    date: r.ann_date.clone(),
                    kind: "repurchase".into(),
                    title: format!(
                        "回购 {} 进度 {}",
                        fmt_num(r.vol_share),
                        r.proc.clone().unwrap_or_default(),
                    ),
                    abstract_text: Some(format!(
                        "金额 {} / 价格上限 {} / 价格下限 {}",
                        fmt_num(r.amount_yuan),
                        fmt_num(r.high_limit),
                        fmt_num(r.low_limit),
                    )),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("repurchase".into());
            }
        } else {
            empty_dimensions.push("repurchase".into());
        }
    }
    if want("institution_visit") {
        if let Some(Ok(rows)) = &visits {
            sources.push("Tushare stk_surv".into());
            let pre = events.len();
            for r in rows {
                events.push(Event {
                    date: r.surv_date.clone(),
                    kind: "institution_visit".into(),
                    title: format!(
                        "调研 {} ({})",
                        r.visiting_org.clone().unwrap_or_default(),
                        r.mode.clone().unwrap_or_default(),
                    ),
                    abstract_text: Some(format!(
                        "接待 {} / 形式 {} / 地点 {} / 来访人 {}",
                        r.host.clone().unwrap_or_default(),
                        r.mode.clone().unwrap_or_default(),
                        r.place.clone().unwrap_or_default(),
                        r.receivers.clone().unwrap_or_default(),
                    )),
                    url: None,
                });
            }
            if events.len() == pre {
                empty_dimensions.push("institution_visit".into());
            }
        } else {
            empty_dimensions.push("institution_visit".into());
        }
    }

    // Sort by date desc.
    events.sort_by(|a, b| b.date.cmp(&a.date));
    events.truncate(80);

    let mut md = format!(
        "## {} 近 {} 天事件流(共 {} 条)\n\n",
        symbol.to_dotted(),
        days,
        events.len()
    );
    for ev in &events {
        md.push_str(&format!(
            "- [{}] **{}** — {}{}\n",
            ev.date,
            ev.kind,
            ev.title,
            ev.url
                .as_deref()
                .map(|u| format!(" — {u}"))
                .unwrap_or_default(),
        ));
    }
    if !empty_dimensions.is_empty() {
        md.push_str(&format!("\n_无数据类型: {}_\n", empty_dimensions.join(", ")));
    }
    if !sources.is_empty() {
        md.push_str(&format!(
            "\n数据来源: {} @ {}\n",
            sources.join("; "),
            today
        ));
    }

    let display = serde_json::json!({
        "kind": "recent_actions",
        "symbol": symbol.to_dotted(),
        "days": days,
        "filter": filters,
        "events": events,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({
        "symbol_input": raw_sym,
        "symbol_normalized": symbol.to_dotted(),
        "days": days,
        "filter": filters,
    });
    ToolOutcome::ok(md, display, debug)
}

fn fmt_num(v: Option<f64>) -> String {
    v.map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into())
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
    fn invalid_days_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "days": 0 }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn invalid_filter_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "filter": ["bogus"] }),
        ));
        assert!(out.is_error);
    }
}
