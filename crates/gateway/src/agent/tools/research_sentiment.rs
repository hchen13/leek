//! `research_sentiment(symbol)` — sell-side facts:
//! consensus forecasts + rating mix + report list + recent broker picks.
//!
//! `report_rc` is rate-limited (1/min upstream) — we hold a process-wide
//! token bucket and surface "skipped due to throttle" as an
//! `empty_dimensions: ['report_rc:throttled']` entry instead of hammering.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorRegistry};

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "research_sentiment".into(),
        description: "Sell-side coverage for one A-share symbol. Returns: \
             consensus forecast (mean / high / low for next 1-2 years' EPS \
             + 净利), rating distribution (买入 / 增持 / 中性 / 减持 / \
             卖出 counts), recent research report list (≤ 30 days, with \
             PDF URLs), and the latest 月度金股 (broker top picks).\n\
             \n\
             Inputs:\n\
             - symbol (required) — A-share symbol.\n\
             \n\
             Examples: '茅台卖方一致预期多少' / '看一下宁德最近研报怎么说'.\n\
             \n\
             Limits: 一致预期接口 1 次 / 分钟全局节流。若 1 分钟内已被\
             调过会 skip 并 surface `empty_dimensions: \
             ['report_rc:throttled']`(model 不重试)。研报列表内含 PDF \
             URL,可交给 `read_pdf` 读全文。\n\
             \n\
             Boundaries: this is the analyst panel. Use `stock_overview` \
             focus='corp_action' for 业绩预告 / 快报,`recent_actions` \
             for 公告 + 调研 timeline。"
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "卖方研报",
        result: ResultArtifact::Card("research_sentiment"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            format!("研报 · {}", super::summary_snippet(s))
        },
    }
}

/// Global throttle for `report_rc` (1/min upstream). We hold the last
/// call's `Instant`; calls within 60 s of the last one are skipped.
static REPORT_RC_THROTTLE: Mutex<Option<Instant>> = Mutex::new(None);
const REPORT_RC_INTERVAL_SECS: u64 = 60;

fn try_acquire_report_rc() -> bool {
    let mut guard = REPORT_RC_THROTTLE.lock().unwrap();
    let now = Instant::now();
    match *guard {
        Some(prev) if now.duration_since(prev).as_secs() < REPORT_RC_INTERVAL_SECS => false,
        _ => {
            *guard = Some(now);
            true
        }
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("research_sentiment: missing 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("research_sentiment: {e}")),
    };
    let t = &vendors.tushare;
    let e = &vendors.eastmoney;
    let today = chrono::Utc::now().naive_utc().date();
    let begin = today - chrono::Duration::days(30);
    let begin_s = begin.format("%Y-%m-%d").to_string();
    let end_s = today.format("%Y-%m-%d").to_string();
    let month_str = today.format("%Y%m").to_string();
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    let throttle_allows = try_acquire_report_rc();

    // Fan-out. `report_rc` only runs when throttle allows; otherwise
    // we surface that to the caller and skip the call entirely.
    let (rc_res, list_t_res, list_e_res, broker_res) = tokio::join!(
        async {
            if throttle_allows {
                Some(t.report_rc(&symbol).await)
            } else {
                None
            }
        },
        t.research_report(&symbol, 30),
        e.research_reports(&symbol, &begin_s, &end_s, 20),
        t.broker_recommend(&month_str),
    );

    let (forecasts, rating_mix, rc_count) = match rc_res {
        Some(Ok(t)) => {
            sources.push(format!("Tushare report_rc @ {today}"));
            t
        }
        Some(Err(e)) => {
            empty_dimensions.push(format!("report_rc:{}", e.message));
            (Vec::new(), Default::default(), 0)
        }
        None => {
            empty_dimensions.push("report_rc:throttled".into());
            (Vec::new(), Default::default(), 0)
        }
    };
    let reports_t = match list_t_res {
        Ok(r) if !r.is_empty() => {
            sources.push(format!("Tushare research_report @ {today}"));
            r
        }
        _ => {
            empty_dimensions.push("research_report".into());
            Vec::new()
        }
    };
    let reports_e = match list_e_res {
        Ok(r) if !r.is_empty() => {
            sources.push(format!("EastMoney reportapi.list @ {today}"));
            r
        }
        _ => {
            empty_dimensions.push("eastmoney_reports".into());
            Vec::new()
        }
    };
    let broker_picks = match broker_res {
        Ok(picks) => {
            sources.push(format!("Tushare broker_recommend @ {today}"));
            // Filter to this symbol only.
            let mine: Vec<_> = picks
                .into_iter()
                .filter(|p| p.ts_code == symbol.to_dotted())
                .collect();
            mine
        }
        Err(_) => {
            empty_dimensions.push("broker_recommend".into());
            Vec::new()
        }
    };

    let mut md = format!("## {} 卖方研报概览\n\n", symbol.to_dotted());
    // Forecasts.
    if !forecasts.is_empty() {
        md.push_str("### 一致预期\n\n| 年份 | EPS 均值 | EPS 高 | EPS 低 | 净利均(亿) | 样本数 |\n|---|---|---|---|---|---|\n");
        for f in &forecasts {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                f.year,
                fmt_num(f.eps_mean),
                fmt_num(f.eps_high),
                fmt_num(f.eps_low),
                fmt_num(f.net_profit_mean_yi_yuan),
                f.sample_size,
            ));
        }
    }
    // Rating mix.
    md.push_str(&format!(
        "\n### 评级分布(本轮 {} 条) — 买入 {} / 增持 {} / 中性 {} / 减持 {} / 卖出 {}\n",
        rc_count, rating_mix.buy, rating_mix.overweight, rating_mix.hold, rating_mix.underweight, rating_mix.sell,
    ));
    // Reports.
    let merged = merge_reports(&reports_t, &reports_e);
    if !merged.is_empty() {
        md.push_str("\n### 近 30 天研报列表\n\n");
        for r in merged.iter().take(15) {
            md.push_str(&format!(
                "- [{}] **{}** — {} ({}) {}\n",
                r.date,
                r.title,
                r.org_name.as_deref().unwrap_or("-"),
                r.author.as_deref().unwrap_or("-"),
                r.pdf_url
                    .as_deref()
                    .map(|u| format!("PDF: {u}"))
                    .unwrap_or_default(),
            ));
        }
        md.push_str(
            "\n_PDF URL 可交给 `read_pdf` 工具读全文。_\n",
        );
    }
    // Broker picks.
    if !broker_picks.is_empty() {
        md.push_str(&format!(
            "\n### 本月券商金股 ({}) — {} 家券商\n\n",
            month_str,
            broker_picks.len()
        ));
        for p in &broker_picks {
            md.push_str(&format!("- {} 推荐 {} ({})\n", p.broker, p.name, p.ts_code));
        }
    }

    if !empty_dimensions.is_empty() {
        md.push_str(&format!("\n_缺失维度: {}_\n", empty_dimensions.join(", ")));
    }
    if !sources.is_empty() {
        md.push_str(&format!("\n数据来源: {}\n", sources.join("; ")));
    }

    let display = serde_json::json!({
        "kind": "research_sentiment",
        "symbol": symbol.to_dotted(),
        "forecasts": forecasts,
        "rating_mix": rating_mix,
        "rc_count": rc_count,
        "reports": merged,
        "broker_picks": broker_picks,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "throttle_allowed": throttle_allows,
    });
    ToolOutcome::ok(md, display, debug)
}

fn merge_reports(
    t: &[crate::vendors::ResearchReportItem],
    e: &[crate::vendors::ResearchReportItem],
) -> Vec<crate::vendors::ResearchReportItem> {
    let mut out: Vec<crate::vendors::ResearchReportItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in e.iter().chain(t.iter()) {
        let key = format!("{}|{}", r.date, r.title);
        if seen.insert(key) {
            out.push(r.clone());
        }
    }
    out.sort_by(|a, b| b.date.cmp(&a.date));
    out
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
    fn throttle_first_call_succeeds_second_skips_within_window() {
        // Reset
        *REPORT_RC_THROTTLE.lock().unwrap() = None;
        assert!(try_acquire_report_rc());
        assert!(!try_acquire_report_rc());
    }

    #[test]
    fn invalid_symbol_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "garbage" }),
        ));
        assert!(out.is_error);
    }
}
