//! `stock_overview` — single-stock dossier.
//!
//! One required `symbol` + optional `focus` enum picks one of seven
//! views: overview / valuation / business / holders / financial /
//! technical / corp_action. Default `overview` is a six-section snapshot
//! suitable for "tell me about this stock" prompts.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorRegistry};

const DEFAULT_FOCUS: &str = "overview";
const FOCUS_CHOICES: &[&str] = &[
    "overview",
    "valuation",
    "business",
    "holders",
    "financial",
    "technical",
    "corp_action",
];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "stock_overview".into(),
        description: "A-share single-stock dossier. One call returns the \
             distilled view for ONE focus area; the canvas card has the \
             raw rows.\n\
             \n\
             Inputs:\n\
             - symbol (required) — `600519.SH` / `600519` / `sh600519`.\n\
             - focus (optional, default 'overview'): \
             'overview' (6-section snapshot: real-time quote + 公司简介 + \
             valuation + industry + concepts + 最近 5 公告), \
             'valuation' (PE/PB/PS/股息率 + 历史 30/50/70 分位), \
             'business' (主营业务构成 by product), \
             'holders' (十大股东 + 十大流通 + 户数 + 实控人 + 机构股东), \
             'financial' (最新一期 income + balance + cashflow + 关键比率), \
             'technical' (MA / RSI / KDJ / MACD / BOLL 原始数值 — \
             agent 自行解读,不在工具内下'超买'判断), \
             'corp_action' (业绩预告 + 业绩快报 + 下次披露日历).\n\
             \n\
             Examples: '茅台怎么样' → focus='overview'; '茅台估值贵不贵' \
             → focus='valuation'; '宁德的股东结构' → focus='holders'.\n\
             \n\
             Limits: returns distilled markdown ≤ 1500 tokens default, \
             ≤ 4000 in focus modes. Per-section vendor outages surface as \
             `empty_dimensions` — do NOT retry, those sections are \
             unavailable for this symbol right now.\n\
             \n\
             Boundaries: this is the single-company panel. Use \
             `industry_landscape` for sector context, `recent_actions` \
             for an event-stream timeline (announcements / 大宗 / 龙虎榜 \
             / 调研 etc.), `research_sentiment` for sell-side consensus \
             + 研报, `chart_data` for raw OHLC + technical numbers."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (`600519.SH` / `600519` / `sh600519`)."
                },
                "focus": {
                    "type": "string",
                    "enum": FOCUS_CHOICES,
                    "description": "Focus area (default 'overview')."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "个股全景",
        result: ResultArtifact::Card("stock_overview"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let f = args
                .get("focus")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_FOCUS);
            format!("个股 · {} · {f}", super::summary_snippet(s))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw_sym) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("stock_overview: missing required 'symbol'.");
    };
    let symbol = match Symbol::parse(raw_sym) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("stock_overview: {e}")),
    };
    let focus = args
        .get("focus")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_FOCUS)
        .to_string();
    if !FOCUS_CHOICES.contains(&focus.as_str()) {
        return ToolOutcome::error(format!(
            "stock_overview: invalid focus '{focus}' (try {})",
            FOCUS_CHOICES.join("/")
        ));
    }
    let t = &vendors.tushare;
    let e = &vendors.eastmoney;
    let today = chrono::Utc::now().naive_utc().date();
    let today_compact = today.format("%Y%m%d").to_string();
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    let (md, display_data) = match focus.as_str() {
        "overview" => {
            // 4 concurrent calls — quote / basic / company / announcements (last 30d).
            let ann_start = (today - chrono::Duration::days(30))
                .format("%Y%m%d")
                .to_string();
            let (quote_res, basic_res, company_res, ann_res) = tokio::join!(
                e.push2_quote(&symbol),
                t.stock_basic_one(&symbol),
                t.stock_company(&symbol),
                t.anns_d(&symbol, &ann_start, &today_compact),
            );
            let (live_quote, eod_fallback) = match quote_res {
                Ok(q) if q.price.is_some() && q.price.unwrap() > 0.0 => (Some(q), false),
                _ => match t.live_quote_eod(&symbol).await {
                    Ok(q) => (Some(q), true),
                    Err(_) => (None, false),
                },
            };
            if live_quote.is_some() {
                if eod_fallback {
                    sources.push(format!("Tushare daily (EOD fallback) @ {today}"));
                } else {
                    sources.push(format!("EastMoney push2 @ {today}"));
                }
            } else {
                empty_dimensions.push("quote".into());
            }
            let basic = match basic_res {
                Ok(b) => {
                    sources.push(format!("Tushare stock_basic @ {today}"));
                    Some(b)
                }
                Err(e) => {
                    empty_dimensions.push(format!("basic:{}", e.message));
                    None
                }
            };
            let company = match company_res {
                Ok(Some(c)) => {
                    sources.push(format!("Tushare stock_company @ {today}"));
                    Some(c)
                }
                _ => {
                    empty_dimensions.push("company".into());
                    None
                }
            };
            let daily_basic = match t.daily_basic_one(&symbol).await {
                Ok(d) => {
                    sources.push(format!("Tushare daily_basic @ {today}"));
                    Some(d)
                }
                Err(_) => {
                    empty_dimensions.push("daily_basic".into());
                    None
                }
            };
            let anns = match ann_res {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare anns_d @ {today}"));
                    rows
                }
                _ => {
                    empty_dimensions.push("announcements".into());
                    Vec::new()
                }
            };

            let mut md = String::new();
            let display_name = basic
                .as_ref()
                .map(|(n, _, _)| n.clone())
                .unwrap_or_default();
            md.push_str(&format!(
                "## {} ({})\n\n",
                display_name,
                symbol.to_dotted()
            ));
            // Section 1: spot quote.
            if let Some(q) = &live_quote {
                md.push_str(&format!(
                    "**行情:** 现价 ¥{} {} (开 {} / 高 {} / 低 {} / 昨收 {})\n\n",
                    fmt_num(q.price),
                    fmt_pct(q.change_pct),
                    fmt_num(q.open),
                    fmt_num(q.high),
                    fmt_num(q.low),
                    fmt_num(q.prev_close),
                ));
            }
            // Section 2: company.
            if let Some(c) = &company {
                let intro = c
                    .introduction
                    .as_deref()
                    .map(|s| s.chars().take(150).collect::<String>())
                    .unwrap_or_default();
                md.push_str(&format!(
                    "**公司:** {} | 主营:{}{}{}\n\n",
                    c.name,
                    c.main_business.as_deref().unwrap_or("-"),
                    if c.employees.is_some() {
                        format!(" | 员工 {} 人", fmt_num(c.employees))
                    } else {
                        String::new()
                    },
                    if !intro.is_empty() {
                        format!("\n\n> {intro}")
                    } else {
                        String::new()
                    },
                ));
            }
            // Section 3: 估值.
            if let Some(db) = &daily_basic {
                let pick = |k: &str| -> Option<f64> { db.get(k).copied().flatten() };
                md.push_str(&format!(
                    "**估值:** PE_TTM={} / PB={} / PS_TTM={} / 股息率={}% / 总市值={} 亿\n\n",
                    fmt_num(pick("pe_ttm")),
                    fmt_num(pick("pb")),
                    fmt_num(pick("ps_ttm")),
                    fmt_num(pick("dv_ttm")),
                    pick("market_cap")
                        .map(|n| format!("{:.0}", n / 1.0e8))
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            // Section 4: 行业.
            if let Some((_, ind, area)) = &basic {
                md.push_str(&format!(
                    "**行业 / 地域:** {} / {}\n\n",
                    ind.clone().unwrap_or_else(|| "-".into()),
                    area.clone().unwrap_or_else(|| "-".into()),
                ));
            }
            // Section 5: 公告.
            if !anns.is_empty() {
                md.push_str("**最近 5 公告:**\n\n");
                for a in anns.iter().take(5) {
                    md.push_str(&format!("- [{}] {}\n", a.date, a.title));
                }
            }
            (
                md,
                serde_json::json!({
                    "quote": live_quote,
                    "basic": basic,
                    "company": company,
                    "daily_basic": daily_basic,
                    "announcements": anns,
                }),
            )
        }
        "valuation" => {
            let (db_res, val_pct_res) = tokio::join!(t.daily_basic_one(&symbol), e.valuation_percentile(&symbol));
            let db = match db_res {
                Ok(d) => {
                    sources.push(format!("Tushare daily_basic @ {today}"));
                    Some(d)
                }
                _ => {
                    empty_dimensions.push("daily_basic".into());
                    None
                }
            };
            let pcts = match val_pct_res {
                Ok(p) if !p.is_empty() => {
                    sources.push(format!("EastMoney RPT_STOCKVALUATIONTANTILE @ {today}"));
                    p
                }
                _ => {
                    empty_dimensions.push("percentile".into());
                    Default::default()
                }
            };
            let mut md = String::new();
            md.push_str(&format!("## {} 估值\n\n", symbol.to_dotted()));
            if let Some(d) = &db {
                let pick = |k: &str| -> Option<f64> { d.get(k).copied().flatten() };
                md.push_str(&format!(
                    "**最新:** PE_TTM={} / PB={} / PS_TTM={} / 股息率={}%\n\n",
                    fmt_num(pick("pe_ttm")),
                    fmt_num(pick("pb")),
                    fmt_num(pick("ps_ttm")),
                    fmt_num(pick("dv_ttm")),
                ));
            }
            md.push_str(&format!(
                "**历史分位(3 年):** PE p30={} / p50={} / p70={} | PB p30={} / p50={} / p70={}\n",
                fmt_num(pcts.get("pe_p30").copied().flatten()),
                fmt_num(pcts.get("pe_p50").copied().flatten()),
                fmt_num(pcts.get("pe_p70").copied().flatten()),
                fmt_num(pcts.get("pb_p30").copied().flatten()),
                fmt_num(pcts.get("pb_p50").copied().flatten()),
                fmt_num(pcts.get("pb_p70").copied().flatten()),
            ));
            (
                md,
                serde_json::json!({ "daily_basic": db, "percentile": pcts }),
            )
        }
        "business" => {
            let rows_res = t.fina_mainbz(&symbol, "product").await;
            let rows = match rows_res {
                Ok(r) => {
                    sources.push(format!("Tushare fina_mainbz @ {today}"));
                    r
                }
                Err(e) => {
                    empty_dimensions.push(format!("fina_mainbz:{}", e.message));
                    Vec::new()
                }
            };
            let mut md = String::new();
            md.push_str(&format!("## {} 主营业务构成 (by product)\n\n", symbol.to_dotted()));
            if let Some(first) = rows.first() {
                md.push_str(&format!("**报告期:** {}\n\n", first.period_end));
                md.push_str("| 业务 | 营收(亿) | 占比 | 毛利率 |\n|---|---|---|---|\n");
                for r in &rows {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} |\n",
                        r.item,
                        fmt_yi(r.revenue_yuan),
                        fmt_pct(r.pct_of_total),
                        fmt_pct(r.gross_margin_pct),
                    ));
                }
            }
            (md, serde_json::json!({ "rows": rows }))
        }
        "holders" => {
            let (total_res, float_res, count_res, controller_res, org_res) = tokio::join!(
                t.top10_holders(&symbol, "total"),
                t.top10_holders(&symbol, "float"),
                t.stk_holdernumber(&symbol, 4),
                e.actual_controller(&symbol),
                e.org_hold_details(&symbol),
            );
            let total = match total_res {
                Ok((end, rows)) => {
                    sources.push(format!("Tushare top10_holders @ {today}"));
                    Some((end, rows))
                }
                _ => {
                    empty_dimensions.push("top10_total".into());
                    None
                }
            };
            let float = match float_res {
                Ok((end, rows)) => {
                    sources.push(format!("Tushare top10_floatholders @ {today}"));
                    Some((end, rows))
                }
                _ => {
                    empty_dimensions.push("top10_float".into());
                    None
                }
            };
            let count_history = match count_res {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare stk_holdernumber @ {today}"));
                    r
                }
                _ => {
                    empty_dimensions.push("holder_count".into());
                    Vec::new()
                }
            };
            let controller = match controller_res {
                Ok(Some(c)) => {
                    sources.push(format!("EastMoney RPT_F10_EH_RELATION @ {today}"));
                    Some(c)
                }
                _ => None,
            };
            let org_holdings = match org_res {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("EastMoney RPT_F10_MAIN_ORGHOLDDETAILS @ {today}"));
                    r
                }
                _ => Vec::new(),
            };
            let mut md = String::new();
            md.push_str(&format!("## {} 股东结构\n\n", symbol.to_dotted()));
            if let Some(c) = &controller {
                md.push_str(&format!("**实控人 / 控股股东:** {c}\n\n"));
            }
            if let Some((end, rows)) = &total {
                md.push_str(&format!("### 十大股东(截至 {})\n\n", end));
                md.push_str("| 排名 | 名称 | 持股(股) | 占比 | QoQ 变动 |\n|---|---|---|---|---|\n");
                for r in rows {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} | {} |\n",
                        r.rank,
                        r.holder_name,
                        fmt_num(r.shares),
                        fmt_pct(r.pct),
                        fmt_num(r.change_qoq_shares),
                    ));
                }
            }
            if let Some((end, rows)) = &float {
                md.push_str(&format!("\n### 十大流通股东(截至 {})\n\n", end));
                md.push_str("| 排名 | 名称 | 持股(股) | 占流通比 |\n|---|---|---|---|\n");
                for r in rows {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} |\n",
                        r.rank,
                        r.holder_name,
                        fmt_num(r.shares),
                        fmt_pct(r.pct),
                    ));
                }
            }
            if !count_history.is_empty() {
                md.push_str("\n### 股东户数变化\n\n| 报告期 | 户数 |\n|---|---|\n");
                for r in &count_history {
                    md.push_str(&format!("| {} | {} |\n", r.end_date, fmt_num(r.holder_count)));
                }
            }
            (
                md,
                serde_json::json!({
                    "total": total,
                    "float": float,
                    "count_history": count_history,
                    "controller": controller,
                    "org_holdings": org_holdings,
                }),
            )
        }
        "financial" => {
            let (income_res, balance_res, cashflow_res, ratios_res) = tokio::join!(
                t.fina_statement(&symbol, "income", "quarter", 4),
                t.fina_statement(&symbol, "balance", "quarter", 4),
                t.fina_statement(&symbol, "cashflow", "quarter", 4),
                t.fina_statement(&symbol, "ratios", "quarter", 4),
            );
            let mut sections: Vec<(&str, Vec<_>)> = Vec::new();
            for (label, res) in [
                ("income", income_res),
                ("balance", balance_res),
                ("cashflow", cashflow_res),
                ("ratios", ratios_res),
            ] {
                match res {
                    Ok(r) if !r.is_empty() => {
                        sources.push(format!("Tushare {label} @ {today}"));
                        sections.push((label, r));
                    }
                    _ => empty_dimensions.push(format!("financial_{label}")),
                }
            }
            let mut md = String::new();
            md.push_str(&format!("## {} 财报快照(最近 4 季度)\n\n", symbol.to_dotted()));
            for (label, rows) in &sections {
                md.push_str(&format!("### {label}\n\n"));
                if let Some(first) = rows.first() {
                    md.push_str(&format!("**最新 {}**\n\n", first.label));
                    md.push_str("| 指标 | 值 |\n|---|---|\n");
                    for (k, v) in &first.metrics {
                        md.push_str(&format!("| {k} | {} |\n", fmt_num(*v)));
                    }
                    md.push('\n');
                }
            }
            (
                md,
                serde_json::json!({
                    "sections": sections.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<std::collections::BTreeMap<_, _>>(),
                }),
            )
        }
        "technical" => {
            let (candles_res, factor_res) = tokio::join!(t.daily(&symbol, 60), t.stk_factor(&symbol, 30));
            let candles = match candles_res {
                Ok(c) if !c.is_empty() => {
                    sources.push(format!("Tushare daily @ {today}"));
                    c
                }
                _ => {
                    empty_dimensions.push("daily".into());
                    Vec::new()
                }
            };
            let factors = match factor_res {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare stk_factor @ {today}"));
                    r
                }
                _ => {
                    empty_dimensions.push("stk_factor".into());
                    Vec::new()
                }
            };
            // Derive MA5 / MA20 / MA60 from candles.
            let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
            let ma = |n: usize| -> Option<f64> {
                if closes.len() < n {
                    return None;
                }
                Some(closes[closes.len() - n..].iter().sum::<f64>() / n as f64)
            };
            let latest = factors.last();
            let mut md = String::new();
            md.push_str(&format!("## {} 技术指标(原始数值)\n\n", symbol.to_dotted()));
            md.push_str(&format!(
                "**均线:** MA5={} / MA20={} / MA60={}\n\n",
                fmt_num(ma(5)),
                fmt_num(ma(20)),
                fmt_num(ma(60)),
            ));
            if let Some(l) = latest {
                md.push_str(&format!(
                    "**最新({}):** RSI6={} / RSI12={} / KDJ K={} D={} J={} / MACD dif={} dea={} hist={} / BOLL up={} mid={} low={}\n",
                    l.date,
                    fmt_num(l.rsi_6),
                    fmt_num(l.rsi_12),
                    fmt_num(l.kdj_k),
                    fmt_num(l.kdj_d),
                    fmt_num(l.kdj_j),
                    fmt_num(l.macd_dif),
                    fmt_num(l.macd_dea),
                    fmt_num(l.macd),
                    fmt_num(l.boll_upper),
                    fmt_num(l.boll_mid),
                    fmt_num(l.boll_lower),
                ));
            }
            md.push_str(
                "\n_所有数值原样,agent 自行判断'超买/超卖/突破'。_\n",
            );
            (
                md,
                serde_json::json!({
                    "candles": candles,
                    "factors": factors,
                    "ma5": ma(5),
                    "ma20": ma(20),
                    "ma60": ma(60),
                }),
            )
        }
        "corp_action" => {
            let (forecast_res, express_res, disclosure_res) = tokio::join!(
                t.forecast(&symbol, 10),
                t.express(&symbol, 10),
                t.disclosure_date(&symbol, 5),
            );
            let forecast = forecast_res.unwrap_or_default();
            let express = express_res.unwrap_or_default();
            let disclosure = disclosure_res.unwrap_or_default();
            if forecast.is_empty() {
                empty_dimensions.push("forecast".into());
            } else {
                sources.push(format!("Tushare forecast @ {today}"));
            }
            if express.is_empty() {
                empty_dimensions.push("express".into());
            } else {
                sources.push(format!("Tushare express @ {today}"));
            }
            if disclosure.is_empty() {
                empty_dimensions.push("disclosure_date".into());
            } else {
                sources.push(format!("Tushare disclosure_date @ {today}"));
            }
            let mut md = String::new();
            md.push_str(&format!("## {} 业绩 / 披露日历\n\n", symbol.to_dotted()));
            if !forecast.is_empty() {
                md.push_str("### 业绩预告(近 10 条)\n\n| 公告日 | 报告期 | 类型 | 净利同比下限% | 净利同比上限% | 摘要 |\n|---|---|---|---|---|---|\n");
                for r in &forecast {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} |\n",
                        r.get("ann_date").and_then(|v| v.as_str()).unwrap_or("-"),
                        r.get("end_date").and_then(|v| v.as_str()).unwrap_or("-"),
                        r.get("type").and_then(|v| v.as_str()).unwrap_or("-"),
                        r.get("p_change_min")
                            .and_then(|v| v.as_f64())
                            .map(|n| format!("{n:+.2}%"))
                            .unwrap_or_else(|| "-".into()),
                        r.get("p_change_max")
                            .and_then(|v| v.as_f64())
                            .map(|n| format!("{n:+.2}%"))
                            .unwrap_or_else(|| "-".into()),
                        r.get("summary")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-")
                            .chars()
                            .take(30)
                            .collect::<String>(),
                    ));
                }
            }
            if !disclosure.is_empty() {
                md.push_str("\n### 下次披露(近 5 条)\n\n| 报告期 | 预计日 | 实际日 |\n|---|---|---|\n");
                for r in &disclosure {
                    md.push_str(&format!(
                        "| {} | {} | {} |\n",
                        r.get("end_date").and_then(|v| v.as_str()).unwrap_or("-"),
                        r.get("pre_date").and_then(|v| v.as_str()).unwrap_or("-"),
                        r.get("actual_date").and_then(|v| v.as_str()).unwrap_or("-"),
                    ));
                }
            }
            (
                md,
                serde_json::json!({
                    "forecast": forecast,
                    "express": express,
                    "disclosure": disclosure,
                }),
            )
        }
        _ => (String::new(), serde_json::json!({})),
    };

    let mut final_md = md;
    if !empty_dimensions.is_empty() {
        final_md.push_str(&format!("\n_缺失维度: {}_\n", empty_dimensions.join(", ")));
    }
    if !sources.is_empty() {
        final_md.push_str(&format!("\n数据来源: {}\n", sources.join("; ")));
    }
    let display = serde_json::json!({
        "kind": "stock_overview",
        "symbol": symbol.to_dotted(),
        "focus": focus,
        "data": display_data,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({
        "symbol_input": raw_sym,
        "symbol_normalized": symbol.to_dotted(),
        "focus": focus,
    });
    ToolOutcome::ok(final_md, display, debug)
}

fn fmt_num(v: Option<f64>) -> String {
    v.map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into())
}
fn fmt_pct(v: Option<f64>) -> String {
    v.map(|n| format!("{n:+.2}%")).unwrap_or_else(|| "-".into())
}
fn fmt_yi(v: Option<f64>) -> String {
    v.map(|n| format!("{:.2}", n / 1.0e8))
        .unwrap_or_else(|| "-".into())
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
            &serde_json::json!({ "symbol": "600519.SH", "focus": "vibes" }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn missing_symbol_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
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
