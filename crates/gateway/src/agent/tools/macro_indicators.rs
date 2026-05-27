//! `macro_indicators` — China + key US macro indicators, facts-only.
//!
//! Inputs:
//!
//! - `focus`: enum (default `inflation`) — `inflation` / `growth` /
//!   `money` / `policy` / `calendar` / `intl_rates`.
//!
//! Each focus mode calls 1-3 upstream macro series concurrently and
//! returns a distilled markdown table. Raw rows go to the canvas card.
//! No interpretation — the agent decides what "rising inflation" /
//! "tight policy" mean.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{
    CpiPpiObs, GdpObs, MacroEventItem, MoneyObs, PmiObs, PolicyItem, ShiborLprObs,
    SocialFinancingObs, UsTbrObs, VendorRegistry,
};

const DEFAULT_FOCUS: &str = "inflation";
const FOCUS_CHOICES: &[&str] = &[
    "inflation",
    "growth",
    "money",
    "policy",
    "calendar",
    "intl_rates",
];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "macro_indicators".into(),
        description: "China macro economic indicators (and a small set of US \
             reference rates). Picks one focus area per call and returns a \
             distilled markdown table with the most recent 12-24 observations \
             plus the latest YoY / MoM numbers up front. Raw rows go to the \
             canvas card.\n\
             \n\
             Inputs:\n\
             - focus (optional, default 'inflation') — one of: 'inflation' \
             (CPI + PPI), 'growth' (GDP + PMI), 'money' (M0/M1/M2 + 社融 + \
             LPR), 'policy' (国务院政策文件库, 近 30 天最多 20 条), \
             'calendar' (中国宏观数据发布日历, 未来 14 天重要指标), \
             'intl_rates' (US T-bill yields).\n\
             \n\
             Examples: 'CPI 怎么样' → focus='inflation'; 'M2 同比多少' → \
             focus='money'; 'GDP 跟 PMI 给我看' → focus='growth'; \
             '近期国务院政策' → focus='policy'; '下周哪些数据公布' → \
             focus='calendar'.\n\
             \n\
             Limits: returns ≤ 24 monthly / 12 quarterly rows; no forecasts. \
             Some focus modes surface `empty_dimensions` when the upstream \
             data set isn't reachable — the model should accept that as a \
             'this section unavailable' signal and NOT retry.\n\
             \n\
             Boundaries: this is the macro panel only. Use `market_overview` \
             for index quotes + capital flow; `industry_landscape` for \
             sector-level facts."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "focus": {
                    "type": "string",
                    "enum": FOCUS_CHOICES,
                    "description": "Focus area (default 'inflation')."
                }
            },
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "宏观指标",
        result: ResultArtifact::Card("macro_indicators"),
        summary: |args| {
            let focus = args
                .get("focus")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_FOCUS);
            format!("宏观 · {focus}")
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let focus = args
        .get("focus")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_FOCUS)
        .to_string();
    if !FOCUS_CHOICES.contains(&focus.as_str()) {
        return ToolOutcome::error(format!(
            "macro_indicators: invalid focus '{focus}' (try {})",
            FOCUS_CHOICES.join("/")
        ));
    }
    let t = &vendors.tushare;
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let today = chrono::Utc::now().naive_utc().date();
    let (model_md, display): (String, serde_json::Value) = match focus.as_str() {
        "inflation" => {
            let (cpi_res, ppi_res) = tokio::join!(t.cn_cpi(24), t.cn_ppi(24));
            let cpi: Vec<CpiPpiObs> = match cpi_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare cn_cpi @ {today}"));
                    rows
                }
                Ok(_) => {
                    empty_dimensions.push("cpi".into());
                    Vec::new()
                }
                Err(e) => {
                    empty_dimensions.push(format!("cpi:{}", e.message));
                    Vec::new()
                }
            };
            let ppi: Vec<CpiPpiObs> = match ppi_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare cn_ppi @ {today}"));
                    rows
                }
                Ok(_) => {
                    empty_dimensions.push("ppi".into());
                    Vec::new()
                }
                Err(e) => {
                    empty_dimensions.push(format!("ppi:{}", e.message));
                    Vec::new()
                }
            };
            let mut md = String::new();
            md.push_str("## 通胀:CPI + PPI\n\n");
            if let Some(latest) = cpi.first() {
                md.push_str(&format!(
                    "**最新:** {} CPI 全国同比 {} / 环比 {}\n\n",
                    latest.month,
                    fmt_pct(latest.nation_yoy),
                    fmt_pct(latest.nation_mom),
                ));
            }
            if let Some(latest) = ppi.first() {
                md.push_str(&format!(
                    "**最新:** {} PPI 全国同比 {} / 月环比 {}\n\n",
                    latest.month,
                    fmt_pct(latest.nation_yoy),
                    fmt_pct(latest.nation_mom),
                ));
            }
            md.push_str("### 近 12 月走势\n\n| 月份 | CPI YoY | CPI MoM | PPI YoY |\n|---|---|---|---|\n");
            for i in 0..12usize {
                let cpi_row = cpi.get(i);
                let ppi_row = ppi.get(i);
                let month = cpi_row
                    .map(|r| r.month.clone())
                    .or_else(|| ppi_row.map(|r| r.month.clone()))
                    .unwrap_or_default();
                if month.is_empty() {
                    break;
                }
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    month,
                    cpi_row.map(|r| fmt_pct(r.nation_yoy)).unwrap_or("-".into()),
                    cpi_row.map(|r| fmt_pct(r.nation_mom)).unwrap_or("-".into()),
                    ppi_row.map(|r| fmt_pct(r.nation_yoy)).unwrap_or("-".into()),
                ));
            }
            (
                md,
                serde_json::json!({
                    "cpi": cpi,
                    "ppi": ppi,
                }),
            )
        }
        "growth" => {
            let (gdp_res, pmi_res) = tokio::join!(t.cn_gdp(8), t.cn_pmi(24));
            let gdp: Vec<GdpObs> = match gdp_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare cn_gdp @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("gdp".into());
                    Vec::new()
                }
            };
            let pmi: Vec<PmiObs> = match pmi_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare cn_pmi @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("pmi".into());
                    Vec::new()
                }
            };
            let mut md = String::new();
            md.push_str("## 增长:GDP + PMI\n\n");
            if let Some(latest) = gdp.first() {
                md.push_str(&format!(
                    "**最新季度:** {} GDP {} 亿元 / 同比 {} (第一产业 {} / 第二 {} / 第三 {})\n\n",
                    latest.quarter,
                    fmt_num(latest.gdp_yi_yuan),
                    fmt_pct(latest.gdp_yoy),
                    fmt_pct(latest.primary_yoy),
                    fmt_pct(latest.secondary_yoy),
                    fmt_pct(latest.tertiary_yoy),
                ));
            }
            if let Some(latest) = pmi.first() {
                md.push_str(&format!(
                    "**最新月度:** {} 制造业 PMI {} / 非制造业 PMI {} (50 = 荣枯线)\n\n",
                    latest.month,
                    fmt_num(latest.manufacturing),
                    fmt_num(latest.non_manufacturing),
                ));
            }
            md.push_str("### PMI 近 12 月\n\n| 月份 | 制造业 | 非制造业 |\n|---|---|---|\n");
            for r in pmi.iter().take(12) {
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    r.month,
                    fmt_num(r.manufacturing),
                    fmt_num(r.non_manufacturing)
                ));
            }
            md.push_str("\n### GDP 近 8 季\n\n| 季度 | GDP YoY |\n|---|---|\n");
            for r in gdp.iter().take(8) {
                md.push_str(&format!("| {} | {} |\n", r.quarter, fmt_pct(r.gdp_yoy)));
            }
            (md, serde_json::json!({ "gdp": gdp, "pmi": pmi }))
        }
        "money" => {
            let (m_res, sf_res, lpr_res) = tokio::join!(t.cn_m(24), t.sf_month(24), t.shibor_lpr(12));
            let m: Vec<MoneyObs> = match m_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare cn_m @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("m".into());
                    Vec::new()
                }
            };
            let sf: Vec<SocialFinancingObs> = match sf_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare sf_month @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("sf_month".into());
                    Vec::new()
                }
            };
            let lpr: Vec<ShiborLprObs> = match lpr_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare shibor_lpr @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("lpr".into());
                    Vec::new()
                }
            };
            let mut md = String::new();
            md.push_str("## 货币 + 社融 + LPR\n\n");
            if let Some(latest) = m.first() {
                md.push_str(&format!(
                    "**最新:** {} M2 同比 {} / M1 同比 {} / M0 同比 {}\n\n",
                    latest.month,
                    fmt_pct(latest.m2_yoy),
                    fmt_pct(latest.m1_yoy),
                    fmt_pct(latest.m0_yoy),
                ));
            }
            if let Some(latest) = sf.first() {
                md.push_str(&format!(
                    "**社融:** {} 新增 {} 亿 / 累计 {} 亿 / 存量 {} 万亿\n\n",
                    latest.month,
                    fmt_num(latest.increment_yi_yuan),
                    fmt_num(latest.cumulative_yi_yuan),
                    fmt_num(latest.stock_wan_yi_yuan),
                ));
            }
            if let Some(latest) = lpr.first() {
                md.push_str(&format!(
                    "**LPR:** {} 1Y={}% 5Y={}%\n\n",
                    latest.date,
                    fmt_num(latest.lpr_1y),
                    fmt_num(latest.lpr_5y),
                ));
            }
            md.push_str("### M 同比近 12 月\n\n| 月份 | M0 YoY | M1 YoY | M2 YoY |\n|---|---|---|---|\n");
            for r in m.iter().take(12) {
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    r.month,
                    fmt_pct(r.m0_yoy),
                    fmt_pct(r.m1_yoy),
                    fmt_pct(r.m2_yoy),
                ));
            }
            (
                md,
                serde_json::json!({ "money": m, "social_financing": sf, "lpr": lpr }),
            )
        }
        "intl_rates" => {
            let tbr_res = t.us_tbr(8).await;
            let tbr: Vec<UsTbrObs> = match tbr_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare us_tbr @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("us_tbr".into());
                    Vec::new()
                }
            };
            let mut md = String::new();
            md.push_str("## 美债收益率\n\n");
            if let Some(latest) = tbr.first() {
                md.push_str(&format!(
                    "**最新:** {} 4W={}% 13W={}% 52W={}%\n\n",
                    latest.date,
                    fmt_num(latest.w4_bd),
                    fmt_num(latest.w13_bd),
                    fmt_num(latest.w52_bd),
                ));
            }
            md.push_str("### 近 8 期\n\n| 日期 | 4W | 13W | 52W |\n|---|---|---|---|\n");
            for r in tbr.iter().take(8) {
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    r.date,
                    fmt_num(r.w4_bd),
                    fmt_num(r.w13_bd),
                    fmt_num(r.w52_bd),
                ));
            }
            (md, serde_json::json!({ "us_tbr": tbr }))
        }
        "policy" => {
            // 30 day window ending today. tushare's `npr` honors
            // start_date / end_date in YYYYMMDD; rows come back
            // newest first.
            let start = (today - chrono::Duration::days(30))
                .format("%Y%m%d")
                .to_string();
            let end = today.format("%Y%m%d").to_string();
            let policies: Vec<PolicyItem> =
                match t.npr(Some(&start), Some(&end), 20).await {
                    Ok(rows) if !rows.is_empty() => {
                        sources.push(format!("Tushare npr @ {today}"));
                        rows
                    }
                    Ok(_) => {
                        empty_dimensions.push("npr".into());
                        Vec::new()
                    }
                    Err(e) => {
                        empty_dimensions.push(format!("npr:{}", e.message));
                        Vec::new()
                    }
                };
            let mut md = String::new();
            md.push_str("## 国务院政策文件库(近 30 天)\n\n");
            if policies.is_empty() {
                md.push_str("窗口内未拉到政策条目。\n");
            } else {
                md.push_str(&format!("共 {} 条,按发布日倒序:\n\n", policies.len()));
                for p in &policies {
                    md.push_str(&format!(
                        "- **{}** · {} · {}{}\n",
                        p.publish_date,
                        p.publish_org.clone().unwrap_or_else(|| "-".into()),
                        p.title,
                        p.policy_id
                            .as_deref()
                            .map(|c| format!(" ({c})"))
                            .unwrap_or_default(),
                    ));
                }
            }
            (md, serde_json::json!({ "policies": policies }))
        }
        "calendar" => {
            // Look at the next 14 days. tushare currently ignores the
            // filter under our token tier, so the adapter falls back
            // to whatever the latest snapshot returns; we surface that
            // honestly rather than fake-empty.
            let start = today.format("%Y%m%d").to_string();
            let end = (today + chrono::Duration::days(14))
                .format("%Y%m%d")
                .to_string();
            let events: Vec<MacroEventItem> =
                match t.cn_schedule(Some(&start), Some(&end), 30).await {
                    Ok(rows) if !rows.is_empty() => {
                        sources.push(format!("Tushare cn_schedule @ {today}"));
                        rows
                    }
                    Ok(_) => {
                        empty_dimensions.push("cn_schedule".into());
                        Vec::new()
                    }
                    Err(e) => {
                        empty_dimensions.push(format!("cn_schedule:{}", e.message));
                        Vec::new()
                    }
                };
            let mut md = String::new();
            md.push_str("## 中国宏观数据发布日历(未来 14 天)\n\n");
            if events.is_empty() {
                md.push_str(
                    "窗口内未拉到日历项(上游 token 当前对 start/end 过滤支持有限)。\n",
                );
            } else {
                md.push_str("| 公布日 | 指标 | 发布单位 | 数据期 | 上游 API |\n|---|---|---|---|---|\n");
                for ev in &events {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} | {} |\n",
                        ev.publish_date,
                        ev.title,
                        ev.issuing_org.clone().unwrap_or_else(|| "-".into()),
                        ev.period.clone().unwrap_or_else(|| "-".into()),
                        ev.data_api.clone().unwrap_or_else(|| "-".into()),
                    ));
                }
            }
            (md, serde_json::json!({ "events": events }))
        }
        // Unreachable because FOCUS_CHOICES validates above, but keep a
        // wildcard so adding a new focus to the enum without updating
        // this match is a compile error (`unreachable_patterns`) only
        // when truly dead.
        _ => unreachable!("focus already validated against FOCUS_CHOICES"),
    };

    let mut md = model_md;
    if !empty_dimensions.is_empty() {
        md.push_str(&format!("\n_缺失维度: {}_\n", empty_dimensions.join(", ")));
    }
    if !sources.is_empty() {
        md.push_str(&format!("\n数据来源: {}\n", sources.join("; ")));
    }
    let display_payload = serde_json::json!({
        "kind": "macro_indicators",
        "focus": focus,
        "data": display,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({ "focus": focus });
    ToolOutcome::ok(md, display_payload, debug)
}

fn fmt_pct(v: Option<f64>) -> String {
    v.map(|n| format!("{n:+.2}%")).unwrap_or_else(|| "-".into())
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
    fn invalid_focus_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "focus": "everything" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("focus"));
    }

    #[test]
    fn empty_dimensions_surface_without_error_when_vendor_silent() {
        // With no tushare token configured, every focus call surfaces
        // empty_dimensions but the tool itself succeeds — the agent
        // sees "this section is unavailable" rather than a recoverable
        // tool-level failure.
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "focus": "inflation" }),
        ));
        assert!(!out.is_error);
        let dims = out.display_payload["empty_dimensions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(!dims.is_empty(), "expected empty_dimensions to be populated");
    }

    #[test]
    fn policy_focus_returns_structured_output_when_vendor_silent() {
        // No tushare token → npr call errors recoverably → policy focus
        // still returns an OK outcome with an empty `policies` array
        // and an `npr:*` entry under `empty_dimensions`. Critically
        // the markdown does NOT claim "no policy published" — it
        // surfaces the access state honestly.
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "focus": "policy" }),
        ));
        assert!(!out.is_error);
        assert_eq!(out.display_payload["focus"], "policy");
        let policies = out.display_payload["data"]["policies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(policies.is_empty());
        let dims: Vec<String> = out.display_payload["empty_dimensions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(dims.iter().any(|d| d.starts_with("npr")));
        assert!(out.model_output.contains("国务院政策文件库"));
    }

    #[test]
    fn calendar_focus_returns_structured_output_when_vendor_silent() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "focus": "calendar" }),
        ));
        assert!(!out.is_error);
        assert_eq!(out.display_payload["focus"], "calendar");
        let dims: Vec<String> = out.display_payload["empty_dimensions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(dims.iter().any(|d| d.starts_with("cn_schedule")));
        assert!(out.model_output.contains("发布日历"));
    }
}
