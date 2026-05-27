//! `market_overview` — whole-market facts: indexes / total flows /
//! hot industries / extreme movers / connect flow.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::VendorRegistry;

const DEFAULT_FOCUS: &str = "snapshot";
const FOCUS_CHOICES: &[&str] = &[
    "snapshot",
    "capital_flow",
    "hot_industries",
    "extreme_movers",
    "north_money",
];
const INDEX_CODES: &[&str] = &["000001.SH", "399001.SZ", "399006.SZ"];
const INDEX_SECIDS_LIVE: &[&str] = &["1.000001", "0.399001", "0.399006"];

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "market_overview".into(),
        description: "Whole-market A-share facts: three major indexes + \
             capital flow + hot industries + limit-up/down movers + \
             northbound flow.\n\
             \n\
             Inputs:\n\
             - focus (optional, default 'snapshot'): 'snapshot' (上证 + \
             深证成指 + 创业板指 spot + 沪深 A 股市值 / 涨跌家数), \
             'capital_flow' (大盘主力净流入 + 北向当日净流入), \
             'hot_industries' (今日行业资金 top/bot 5), \
             'extreme_movers' (涨停 / 跌停 / 连板梯队), \
             'north_money' (北向当日 + caveat:个股 2024-08-19 起停披露).\n\
             \n\
             Examples: '大盘怎么样' → focus='snapshot'; '今天主力进出多少' \
             → focus='capital_flow'; '涨停几家' → focus='extreme_movers'.\n\
             \n\
             Limits: trade-time spot prices via push2 (free, public); \
             post-close use EoD. Northbound logic: 港交所自 2024-08-19 \
             停披露逐日个股北向 — 工具只给当日总流向 + 季频持股,绝不\
             伪造个股日频。\n\
             \n\
             Boundaries: this is index + aggregates only. Use \
             `industry_landscape` for one sector; `stock_overview` for \
             one company."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "focus": {
                    "type": "string",
                    "enum": FOCUS_CHOICES,
                    "description": "Focus area (default 'snapshot')."
                }
            },
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "大盘全景",
        result: ResultArtifact::Card("market_overview"),
        summary: |args| {
            let f = args
                .get("focus")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_FOCUS);
            format!("大盘 · {f}")
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
            "market_overview: invalid focus '{focus}' (try {})",
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
        "snapshot" => {
            // Fan out three concurrent queries.
            let (idx_live, idx_basic, daily_info) = tokio::join!(
                e.push2_indexes(INDEX_SECIDS_LIVE),
                t.index_dailybasic(INDEX_CODES, Some(&today_compact)),
                t.daily_info(Some(&today_compact)),
            );
            let live = match idx_live {
                Ok(v) if !v.is_empty() => {
                    sources.push(format!("EastMoney push2 indexes @ {today}"));
                    v
                }
                _ => {
                    empty_dimensions.push("indexes_live".into());
                    Vec::new()
                }
            };
            let basics = match idx_basic {
                Ok(v) if !v.is_empty() => {
                    sources.push(format!("Tushare index_dailybasic @ {today}"));
                    v
                }
                _ => {
                    empty_dimensions.push("indexes_basic".into());
                    Vec::new()
                }
            };
            let totals = match daily_info {
                Ok(v) if !v.is_empty() => {
                    sources.push(format!("Tushare daily_info @ {today}"));
                    v
                }
                _ => {
                    empty_dimensions.push("market_totals".into());
                    Vec::new()
                }
            };
            let mut md = String::new();
            md.push_str("## 三大指数实时\n\n| 代码 | 名称 | 现价 | 涨跌% | 成交额(元) |\n|---|---|---|---|---|\n");
            for q in &live {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    q.symbol,
                    q.name.clone().unwrap_or_default(),
                    fmt_num(q.price),
                    fmt_pct(q.change_pct),
                    fmt_yi(q.turnover_yuan),
                ));
            }
            if !totals.is_empty() {
                md.push_str("\n## 沪深 A/B 股每日统计\n\n| 市场 | 公司数 | 总市值(亿) | 成交额(亿) | 平均 PE | 换手率 |\n|---|---|---|---|---|---|\n");
                for r in &totals {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} |\n",
                        r.ts_name,
                        fmt_num(r.com_count),
                        fmt_num(r.total_mv_yi_yuan),
                        fmt_num(r.amount_yi_yuan),
                        fmt_num(r.pe),
                        fmt_num(r.turnover_rate),
                    ));
                }
            }
            if !basics.is_empty() {
                md.push_str("\n## 指数估值(对应指数)\n\n| 代码 | PE_TTM | PB | 换手率 |\n|---|---|---|---|\n");
                for b in &basics {
                    md.push_str(&format!(
                        "| {} | {} | {} | {} |\n",
                        b.ts_code,
                        fmt_num(b.pe_ttm),
                        fmt_num(b.pb),
                        fmt_num(b.turnover_rate),
                    ));
                }
            }
            (
                md,
                serde_json::json!({
                    "indexes_live": live,
                    "indexes_basic": basics,
                    "market_totals": totals,
                }),
            )
        }
        "capital_flow" => {
            let (mkt_dc, hsgt) = tokio::join!(
                t.moneyflow_mkt_dc(Some(&today_compact)),
                t.moneyflow_hsgt(Some(&today_compact)),
            );
            let mkt = match mkt_dc {
                Ok(Some(m)) => {
                    sources.push(format!("Tushare moneyflow_mkt_dc @ {today}"));
                    Some(m)
                }
                _ => {
                    empty_dimensions.push("market_flow".into());
                    None
                }
            };
            let connect = match hsgt {
                Ok(Some(h)) => {
                    sources.push(format!("Tushare moneyflow_hsgt @ {today}"));
                    Some(h)
                }
                _ => {
                    empty_dimensions.push("hsgt".into());
                    None
                }
            };
            let mut md = String::new();
            md.push_str("## 大盘资金面\n\n");
            if let Some(m) = &mkt {
                let pick = |k: &str| -> Option<f64> { m.get(k).copied().flatten() };
                md.push_str(&format!(
                    "- 净流入:{}\n- 超大单:{}\n- 大单:{}\n- 中单:{}\n- 小单:{}\n",
                    fmt_yi(pick("net_amount")),
                    fmt_yi(pick("buy_elg_amount")),
                    fmt_yi(pick("buy_lg_amount")),
                    fmt_yi(pick("buy_md_amount")),
                    fmt_yi(pick("buy_sm_amount")),
                ));
            }
            if let Some(c) = &connect {
                md.push_str(&format!(
                    "\n## 沪深港通\n\n- 北向当日净额:{} 万元 (沪 {} / 深合计)\n- 南向当日净额:{} 万元\n",
                    fmt_num(c.north_money_wan),
                    fmt_num(c.hgt_wan),
                    fmt_num(c.south_money_wan),
                ));
            }
            (
                md,
                serde_json::json!({
                    "market_flow": mkt,
                    "hsgt": connect,
                }),
            )
        }
        "hot_industries" => {
            let rows = match t.moneyflow_ind_dc(Some(&today_compact), 100).await {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare moneyflow_ind_dc @ {today}"));
                    r
                }
                _ => {
                    empty_dimensions.push("ind_flow".into());
                    Vec::new()
                }
            };
            let top: Vec<_> = rows.iter().take(5).cloned().collect();
            let mut sorted = rows.clone();
            sorted.sort_by(|a, b| {
                a.net_amount_yuan
                    .unwrap_or(0.0)
                    .partial_cmp(&b.net_amount_yuan.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let bottom: Vec<_> = sorted.iter().take(5).cloned().collect();
            let mut md = String::new();
            md.push_str("## 今日行业资金 top/bottom 5\n\n### 流入 top 5\n\n| 排名 | 行业 | 涨跌% | 净流入(亿) | 龙头 |\n|---|---|---|---|---|\n");
            for r in &top {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    r.rank.unwrap_or(0),
                    r.name,
                    fmt_pct(r.pct_change),
                    fmt_yi(r.net_amount_yuan),
                    r.lead_stock.clone().unwrap_or_else(|| "-".into()),
                ));
            }
            md.push_str("\n### 流出 bot 5\n\n| 行业 | 涨跌% | 净流出(亿) |\n|---|---|---|\n");
            for r in &bottom {
                md.push_str(&format!(
                    "| {} | {} | {} |\n",
                    r.name,
                    fmt_pct(r.pct_change),
                    fmt_yi(r.net_amount_yuan),
                ));
            }
            (
                md,
                serde_json::json!({ "top": top, "bottom": bottom }),
            )
        }
        "extreme_movers" => {
            let (limit_d, limit_c) = tokio::join!(
                t.limit_list_d(&today_compact),
                t.limit_cpt_list(&today_compact),
            );
            let limit_list = match limit_d {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare limit_list_d @ {today}"));
                    r
                }
                _ => {
                    empty_dimensions.push("limit_list_d".into());
                    Vec::new()
                }
            };
            let limit_cpt = match limit_c {
                Ok(r) if !r.is_empty() => {
                    sources.push(format!("Tushare limit_cpt_list @ {today}"));
                    r
                }
                _ => {
                    empty_dimensions.push("limit_cpt_list".into());
                    Vec::new()
                }
            };
            let mut up = 0usize;
            let mut down = 0usize;
            let mut zha = 0usize;
            for r in &limit_list {
                match r.limit.as_deref() {
                    Some("U") => up += 1,
                    Some("D") => down += 1,
                    Some("Z") => zha += 1,
                    _ => {}
                }
            }
            let mut md = String::new();
            md.push_str(&format!(
                "## 涨跌停 + 连板\n\n- 涨停 {} 家 / 跌停 {} 家 / 炸板 {} 家\n\n",
                up, down, zha
            ));
            md.push_str("### 强势连板梯队\n\n| 排名 | 板块/股票 | 连板 | 板成员涨停数 |\n|---|---|---|---|\n");
            for r in limit_cpt.iter().take(10) {
                md.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    r.up_nums.unwrap_or(0),
                    r.name,
                    r.up_stat.clone().unwrap_or_default(),
                    r.cons_nums.clone().unwrap_or_default(),
                ));
            }
            (
                md,
                serde_json::json!({
                    "up_limit_count": up,
                    "down_limit_count": down,
                    "fan_open_count": zha,
                    "limit_list": limit_list,
                    "concept_cohort": limit_cpt,
                }),
            )
        }
        "north_money" => {
            let hsgt_res = t.moneyflow_hsgt(Some(&today_compact)).await;
            let kamt_res = e.kamt().await;
            let connect = match hsgt_res {
                Ok(Some(h)) => {
                    sources.push(format!("Tushare moneyflow_hsgt @ {today}"));
                    Some(h)
                }
                _ => {
                    empty_dimensions.push("hsgt".into());
                    None
                }
            };
            let live = match kamt_res {
                Ok(Some(k)) => {
                    sources.push(format!("EastMoney kamt @ {today}"));
                    Some(k)
                }
                _ => {
                    empty_dimensions.push("kamt".into());
                    None
                }
            };
            let mut md = String::new();
            md.push_str("## 北向资金\n\n");
            md.push_str("> **数据警告:** 港交所自 2024-08-19 起停披露逐日个股北向成交净额,仅当日合计 + 季频持股可用。\n\n");
            if let Some(c) = &connect {
                md.push_str(&format!(
                    "- 北向当日总净额:{} 万元\n- 沪股通 {} / 深股通 {}\n",
                    fmt_num(c.north_money_wan),
                    fmt_num(c.hgt_wan),
                    fmt_num(c.sgt_wan),
                ));
            }
            if let Some(l) = &live {
                md.push_str(&format!(
                    "\n- 实时累计(东财):北向 {} 万元 / 南向 {} 万元\n",
                    fmt_num(l.north_money_wan),
                    fmt_num(l.south_money_wan),
                ));
            }
            (
                md,
                serde_json::json!({
                    "hsgt": connect,
                    "kamt_live": live,
                    "caveat": "Per-stock daily north flow disabled since 2024-08-19",
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
    final_md.push_str(focus_hint_for(&focus));
    let display = serde_json::json!({
        "kind": "market_overview",
        "focus": focus,
        "data": display_data,
        "empty_dimensions": empty_dimensions,
        "sources": sources,
    });
    let debug = serde_json::json!({ "focus": focus });
    ToolOutcome::ok(final_md, display, debug)
}

/// Append a "其他视角" cross-link block to every focus output. The
/// snapshot focus (default) gets the full menu so the agent learns the
/// whole tool surface on its first call. Every other focus gets a
/// shorter pointer back to the siblings — one-line nudges, not full
/// menus, to stay out of the way.
fn focus_hint_for(focus: &str) -> &'static str {
    match focus {
        "snapshot" => {
            "\n---\n\
             **其他视角**(用同一工具不同 focus):\n\
             - 主力资金 / 北向当日:`market_overview(focus=\"capital_flow\")`\n\
             - 行业 top/bot 5:`market_overview(focus=\"hot_industries\")`\n\
             - 涨停 / 跌停 / 连板:`market_overview(focus=\"extreme_movers\")`\n\
             - 北向资金(2024-08-19 起逐日个股停披露,仅当日合计 + 季频持股):\
             `market_overview(focus=\"north_money\")`\n"
        }
        "capital_flow" => {
            "\n---\n\
             **互为补充**:`focus=\"hot_industries\"` 看资金落在哪些行业; \
             `focus=\"north_money\"` 看北向当日明细; `focus=\"snapshot\"` 回到指数 + 涨跌家数。\n"
        }
        "hot_industries" => {
            "\n---\n\
             **互为补充**:行业资金背后看大盘资金面用 `focus=\"capital_flow\"`; \
             看具体行业内龙头股用 `industry_landscape(target=\"<行业名>\")`。\n"
        }
        "extreme_movers" => {
            "\n---\n\
             **互为补充**:`focus=\"hot_industries\"` 看涨停板背后的行业资金; \
             `focus=\"snapshot\"` 看大盘整体气氛。\n"
        }
        "north_money" => {
            "\n---\n\
             **互为补充**:`focus=\"capital_flow\"` 看大盘整体资金面; \
             个股北向只剩季频(港交所 2024-08-19 起停披露逐日),工具不会伪造。\n"
        }
        _ => "",
    }
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
            &serde_json::json!({ "focus": "vibes" }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn focus_hint_snapshot_full_menu() {
        // Direct unit test for the cross-link block (no HTTP / runtime
        // dance). Run-path coverage of `snapshot` is exercised end-to-end
        // by the PM acceptance prompts.
        let h = focus_hint_for("snapshot");
        assert!(h.contains("其他视角"));
        assert!(h.contains("focus=\"capital_flow\""));
        assert!(h.contains("focus=\"hot_industries\""));
        assert!(h.contains("focus=\"extreme_movers\""));
        assert!(h.contains("focus=\"north_money\""));
    }

    #[test]
    fn focus_hint_non_snapshot_short_hint() {
        for f in ["capital_flow", "hot_industries", "extreme_movers", "north_money"] {
            let h = focus_hint_for(f);
            assert!(h.contains("互为补充"), "focus={f} missing cross-link");
        }
        // Unknown focus → empty (nothing to append).
        assert!(focus_hint_for("zzz_unknown").is_empty());
    }
}
