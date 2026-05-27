//! `industry_landscape` — sector-level facts (leaders, valuation
//! distribution, capital flow ranking, related concepts/indexes).

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{ConceptItem, IndustryFlowRow, IndustryLeaderRow, Symbol, VendorRegistry};

const DEFAULT_FOCUS: &str = "leaders";
const FOCUS_CHOICES: &[&str] = &[
    "leaders",
    "valuation",
    "capital_flow",
    "concepts",
    "index",
];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "industry_landscape".into(),
        description: "Single-industry facts: top-10 leaders + valuation \
             distribution + capital flow ranking + related concept boards \
             + industry index quote.\n\
             \n\
             Inputs:\n\
             - target (required) — either a stock symbol (the tool will \
               look up its industry, e.g. '600519.SH' → '白酒') or a \
               literal industry name in Chinese (e.g. '白酒', '半导体', \
               '工业金属').\n\
             - focus (optional, default 'leaders'): 'leaders' (top-10 \
               companies + market cap + PE/PB/ROE), 'valuation' (industry \
               PE/PB median + dispersion), 'capital_flow' (today's main \
               net inflow ranking + the target industry's rank), \
               'concepts' (concept boards whose name overlaps the \
               industry, ranked by daily heat, with the lead stock for \
               each), 'index' (industry-index quote).\n\
             \n\
             Examples: '茅台所在行业的龙头有哪些' → \
             target='600519.SH', focus='leaders'; '工业金属今天主力进出多少' \
             → target='工业金属', focus='capital_flow'.\n\
             \n\
             Limits: returns ≤ 10 leader rows + ≤ 20 capital-flow rows. \
             Industry classification fidelity depends on upstream — names \
             that don't match the upstream taxonomy may surface \
             `empty_dimensions: ['industry_unknown']`.\n\
             \n\
             Boundaries: this is sector-level facts. Use `stock_overview` \
             for one company; `market_overview` for the whole market; \
             `macro_indicators` for top-down economic context."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "Stock symbol (auto-resolved to industry) or industry name in Chinese."
                },
                "focus": {
                    "type": "string",
                    "enum": FOCUS_CHOICES,
                    "description": "Focus area (default 'leaders')."
                }
            },
            "required": ["target"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "行业全景",
        result: ResultArtifact::Card("industry_landscape"),
        summary: |args| {
            let t = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
            let f = args
                .get("focus")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_FOCUS);
            format!("行业 · {} · {f}", super::summary_snippet(t))
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(target) = args.get("target").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("industry_landscape: missing 'target' (symbol or industry name).");
    };
    let focus = args
        .get("focus")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_FOCUS)
        .to_string();
    if !FOCUS_CHOICES.contains(&focus.as_str()) {
        return ToolOutcome::error(format!(
            "industry_landscape: invalid focus '{focus}' (try {})",
            FOCUS_CHOICES.join("/")
        ));
    }
    let t = &vendors.tushare;
    let mut empty_dimensions: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let today = chrono::Utc::now().naive_utc().date();
    let today_compact = today.format("%Y%m%d").to_string();

    // Resolve target → industry name.
    let industry = match Symbol::parse(target) {
        Ok(sym) => match t.stock_basic_one(&sym).await {
            Ok((_, Some(ind), _)) => {
                sources.push(format!("Tushare stock_basic @ {today}"));
                ind
            }
            Ok((_, None, _)) => {
                return ToolOutcome::ok(
                    format!("## {target}\n\n上游未给该 symbol 的行业分类。\n"),
                    serde_json::json!({
                        "kind": "industry_landscape",
                        "target": target,
                        "focus": focus,
                        "empty_dimensions": ["industry_unknown"],
                        "sources": sources,
                    }),
                    serde_json::json!({ "target": target }),
                );
            }
            Err(e) => {
                return ToolOutcome::ok(
                    format!("## {target}\n\n查询行业失败: {}\n", e.message),
                    serde_json::json!({
                        "kind": "industry_landscape",
                        "target": target,
                        "focus": focus,
                        "empty_dimensions": [format!("stock_basic:{}", e.message)],
                        "sources": sources,
                    }),
                    serde_json::json!({ "target": target }),
                );
            }
        },
        Err(_) => target.to_string(),
    };

    let mut md = format!("## 行业:{industry} · focus={focus}\n\n");
    let display_data = match focus.as_str() {
        "leaders" => {
            let peers_res = t.stock_basic_for_industry(&industry).await;
            let peers: Vec<(String, String)> = match peers_res {
                Ok(p) if !p.is_empty() => p,
                Ok(_) => {
                    empty_dimensions.push("peers".into());
                    Vec::new()
                }
                Err(e) => {
                    empty_dimensions.push(format!("peers:{}", e.message));
                    Vec::new()
                }
            };
            // Enrich up to 10 with daily_basic.
            let mut leaders: Vec<IndustryLeaderRow> = Vec::new();
            for (ts, name) in peers.iter().take(12) {
                if let Some((_ts2, row)) =
                    t.daily_basic_bulk(std::slice::from_ref(ts)).await.into_iter().next()
                {
                    leaders.push(IndustryLeaderRow {
                        ts_code: ts.clone(),
                        name: name.clone(),
                        total_mv_yi_yuan: row
                            .get("market_cap")
                            .and_then(|v| *v)
                            .map(|v| v / 1.0e8),
                        pe_ttm: row.get("pe_ttm").and_then(|v| *v),
                        pb: row.get("pb").and_then(|v| *v),
                        dv_ttm: row.get("dv_ttm").and_then(|v| *v),
                        turnover_rate: row.get("turnover_rate").and_then(|v| *v),
                    });
                }
            }
            sources.push(format!("Tushare daily_basic @ {today}"));
            // Sort by mkt cap desc, keep top 10.
            leaders.sort_by(|a, b| {
                b.total_mv_yi_yuan
                    .unwrap_or(0.0)
                    .partial_cmp(&a.total_mv_yi_yuan.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            leaders.truncate(10);
            md.push_str(&format!("**{industry} 行业 top 10 龙头(按市值)**\n\n"));
            md.push_str("| 代码 | 名称 | 市值(亿) | PE | PB | 股息率 | 换手率 |\n|---|---|---|---|---|---|---|\n");
            for r in &leaders {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} |\n",
                    r.ts_code,
                    r.name,
                    fmt_num(r.total_mv_yi_yuan),
                    fmt_num(r.pe_ttm),
                    fmt_num(r.pb),
                    fmt_num(r.dv_ttm),
                    fmt_num(r.turnover_rate),
                ));
            }
            serde_json::json!({ "leaders": leaders })
        }
        "valuation" => {
            let peers = t.stock_basic_for_industry(&industry).await.unwrap_or_default();
            let codes: Vec<String> = peers.iter().take(40).map(|(c, _)| c.clone()).collect();
            let rows = t.daily_basic_bulk(&codes).await;
            sources.push(format!("Tushare daily_basic (bulk) @ {today}"));
            let mut pe: Vec<f64> = Vec::new();
            let mut pb: Vec<f64> = Vec::new();
            for (_, r) in &rows {
                if let Some(Some(v)) = r.get("pe_ttm") {
                    if *v > 0.0 && *v < 1000.0 {
                        pe.push(*v);
                    }
                }
                if let Some(Some(v)) = r.get("pb") {
                    if *v > 0.0 && *v < 50.0 {
                        pb.push(*v);
                    }
                }
            }
            let pe_med = median(&mut pe);
            let pb_med = median(&mut pb);
            md.push_str(&format!(
                "**{industry} 估值分布(基于 {} 家成份股)**\n\n- PE 中位:{}\n- PB 中位:{}\n",
                rows.len(),
                fmt_num(pe_med),
                fmt_num(pb_med),
            ));
            if rows.is_empty() {
                empty_dimensions.push("valuation_rows".into());
            }
            serde_json::json!({
                "sample_size": rows.len(),
                "pe_ttm_median": pe_med,
                "pb_median": pb_med,
            })
        }
        "capital_flow" => {
            let rows = match t.moneyflow_ind_dc(Some(&today_compact), 30).await {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare moneyflow_ind_dc @ {today}"));
                    r
                }
                _ => {
                    // Trading day may be empty; try yesterday.
                    let yest = (today - chrono::Duration::days(1))
                        .format("%Y%m%d")
                        .to_string();
                    match t.moneyflow_ind_dc(Some(&yest), 30).await {
                        Ok(r) if !r.is_empty() => {
                            sources.push(format!("Tushare moneyflow_ind_dc @ {yest}"));
                            r
                        }
                        _ => {
                            empty_dimensions.push("ind_flow".into());
                            Vec::<IndustryFlowRow>::new()
                        }
                    }
                }
            };
            let target_row = rows.iter().find(|r| r.name.contains(&industry)).cloned();
            md.push_str("**今日行业主力净流入 top 10**\n\n");
            md.push_str("| 排名 | 行业 | 涨跌% | 净流入(亿元) | 净流入占比 | 龙头 |\n|---|---|---|---|---|---|\n");
            for r in rows.iter().take(10) {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    r.rank.unwrap_or(0),
                    r.name,
                    fmt_pct(r.pct_change),
                    fmt_yi(r.net_amount_yuan),
                    fmt_pct(r.net_amount_rate),
                    r.lead_stock.clone().unwrap_or_else(|| "-".into()),
                ));
            }
            if let Some(t) = &target_row {
                md.push_str(&format!(
                    "\n**{industry} 当日:** rank={} 净流入 {} 亿\n",
                    t.rank.unwrap_or(0),
                    fmt_yi(t.net_amount_yuan)
                ));
            }
            serde_json::json!({
                "top10": rows.iter().take(10).cloned().collect::<Vec<_>>(),
                "target": target_row,
            })
        }
        "concepts" => {
            // Two-step fan-out: pull the full concept universe (5000+
            // rows), score each by substring overlap with the industry
            // name, keep the top-N hottest matches. We don't call
            // `dc_concept_cons` per match — concept rows already carry
            // `lead_stock`, and the constituent count is a separate
            // call we skip to keep latency bounded.
            const TOP_N: usize = 10;
            let concepts_all = match t.dc_concept(6000).await {
                Ok(rows) if !rows.is_empty() => {
                    sources.push(format!("Tushare dc_concept @ {today}"));
                    rows
                }
                Ok(_) => {
                    empty_dimensions.push("dc_concept".into());
                    Vec::new()
                }
                Err(e) => {
                    empty_dimensions.push(format!("dc_concept:{}", e.message));
                    Vec::new()
                }
            };

            // Build a small candidate set: prefer keyword overlap with
            // the industry name. Fall back to first 30 concepts when no
            // keyword match exists so the agent at least sees the
            // hottest boards of the day.
            let keyword = industry_keyword(&industry);
            let mut candidates: Vec<ConceptItem> = concepts_all
                .iter()
                .filter(|c| !keyword.is_empty() && c.name.contains(&keyword))
                .cloned()
                .collect();
            let fell_back = candidates.is_empty();
            if fell_back && !concepts_all.is_empty() {
                empty_dimensions.push("concept_keyword_match".into());
            }
            if fell_back {
                candidates = concepts_all.iter().take(30).cloned().collect();
            }
            // Sort by heat (desc) — higher hot is hotter; missing hot
            // sorts last.
            candidates.sort_by(|a, b| {
                b.hot
                    .unwrap_or(f64::MIN)
                    .partial_cmp(&a.hot.unwrap_or(f64::MIN))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let picks: Vec<ConceptItem> = candidates.into_iter().take(TOP_N).collect();

            md.push_str(&if fell_back && !picks.is_empty() {
                format!(
                    "**与 '{industry}' 关键字匹配为空 — fall back: 当日全市场最热 {} 个概念板**\n\n",
                    picks.len()
                )
            } else if !picks.is_empty() {
                format!(
                    "**与 '{industry}' 名字相关的概念板(按当日 hot 倒序,取前 {})**\n\n",
                    picks.len()
                )
            } else {
                format!("**'{industry}' 未匹配到任何概念板。**\n\n")
            });
            if !picks.is_empty() {
                md.push_str("| 概念名 | hot | 当日% | 龙头 | 龙头% | 涨停成员 |\n|---|---|---|---|---|---|\n");
                for c in &picks {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} |\n",
                        c.name,
                        fmt_num(c.hot),
                        fmt_pct(c.pct_change),
                        c.lead_stock.clone().unwrap_or_else(|| "-".into()),
                        fmt_pct(c.lead_stock_pct),
                        c.z_t_num
                            .map(|n| format!("{}", n as i64))
                            .unwrap_or_else(|| "-".into()),
                    ));
                }
            }
            serde_json::json!({
                "concepts": picks,
                "fell_back_to_market_hot": fell_back,
                "keyword_used": keyword,
            })
        }
        "index" => {
            let rows = match t.sw_daily(&today_compact).await {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare sw_daily @ {today}"));
                    r
                }
                _ => {
                    empty_dimensions.push("sw_daily".into());
                    Vec::new()
                }
            };
            let matching: Vec<_> = rows
                .iter()
                .filter(|r| {
                    r.get("name")
                        .and_then(|v| v.as_str())
                        .map(|n| n.contains(&industry))
                        .unwrap_or(false)
                })
                .collect();
            md.push_str(&format!("**申万行业指数:与 '{industry}' 匹配 {} 条**\n\n", matching.len()));
            md.push_str("| 代码 | 名称 | 收盘 | 涨跌% | PE | PB |\n|---|---|---|---|---|---|\n");
            for r in &matching {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    r.get("ts_code").and_then(|v| v.as_str()).unwrap_or("-"),
                    r.get("name").and_then(|v| v.as_str()).unwrap_or("-"),
                    r.get("close")
                        .and_then(|v| v.as_f64())
                        .map(|n| format!("{n:.2}"))
                        .unwrap_or_else(|| "-".into()),
                    r.get("pct_change")
                        .and_then(|v| v.as_f64())
                        .map(|n| format!("{n:+.2}%"))
                        .unwrap_or_else(|| "-".into()),
                    r.get("pe")
                        .and_then(|v| v.as_f64())
                        .map(|n| format!("{n:.2}"))
                        .unwrap_or_else(|| "-".into()),
                    r.get("pb")
                        .and_then(|v| v.as_f64())
                        .map(|n| format!("{n:.2}"))
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            serde_json::json!({ "matching_indices": matching })
        }
        _ => serde_json::json!({}),
    };

    if !empty_dimensions.is_empty() {
        md.push_str(&format!("\n_缺失维度: {}_\n", empty_dimensions.join(", ")));
    }
    if !sources.is_empty() {
        md.push_str(&format!("\n数据来源: {}\n", sources.join("; ")));
    }
    let display = serde_json::json!({
        "kind": "industry_landscape",
        "target": target,
        "industry": industry,
        "focus": focus,
        "data": display_data,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({ "target": target, "focus": focus });
    ToolOutcome::ok(md, display, debug)
}

/// Strip the SW industry-name suffixes the upstream taxonomy uses
/// (`Ⅱ`, `Ⅲ`, `2`, etc.) so the keyword survives a substring match
/// against EastMoney concept names. We keep the result on the original
/// string when it doesn't end in a known suffix.
fn industry_keyword(industry: &str) -> String {
    let mut s = industry.trim().to_string();
    // Trim trailing Roman / Arabic tier markers like `白酒Ⅱ` → `白酒`.
    while let Some(last) = s.chars().last() {
        let is_tier = matches!(last, 'Ⅰ' | 'Ⅱ' | 'Ⅲ' | 'Ⅳ' | 'Ⅴ')
            || (last.is_ascii_digit() && s.chars().count() > 2);
        if !is_tier {
            break;
        }
        s.pop();
    }
    s.trim().to_string()
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
fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
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
            &serde_json::json!({ "target": "白酒", "focus": "vibes" }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn missing_target_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(&vendors, &serde_json::json!({})));
        assert!(out.is_error);
    }

    #[test]
    fn industry_keyword_strips_sw_tier_suffixes() {
        assert_eq!(industry_keyword("白酒Ⅱ"), "白酒");
        assert_eq!(industry_keyword("半导体Ⅲ"), "半导体");
        assert_eq!(industry_keyword("工业金属"), "工业金属");
        // Don't strip when the suffix is meaningful (3-char name).
        assert_eq!(industry_keyword("化学制品"), "化学制品");
    }

    #[test]
    fn concepts_focus_returns_structured_output_when_vendor_silent() {
        // No tushare token → industry resolution via stock_basic errors,
        // returning the structured "industry_unknown / stock_basic:..."
        // payload before we even reach dc_concept. Either way the tool
        // must come back ok=true with a kind=industry_landscape envelope
        // and surface a non-error model output.
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "target": "白酒", "focus": "concepts" }),
        ));
        assert!(!out.is_error);
        assert_eq!(out.display_payload["kind"], "industry_landscape");
        assert_eq!(out.display_payload["focus"], "concepts");
        let dims: Vec<String> = out.display_payload["empty_dimensions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(
            dims.iter().any(|d| d.starts_with("dc_concept")),
            "expected dc_concept entry in empty_dimensions, got {dims:?}"
        );
    }
}
