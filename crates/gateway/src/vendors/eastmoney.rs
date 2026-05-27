//! EastMoney HTTP adapter (M4.1.1).
//!
//! Lookup surfaces:
//!
//! - `push2.eastmoney.com/api/qt/stock/get` — single-symbol live quote
//! - `push2.eastmoney.com/api/qt/ulist.np/get` — batch live capital flow
//! - `push2his.eastmoney.com/api/qt/stock/kline/get` — daily K-line
//! - `datacenter-web.eastmoney.com/api/data/v1/get` — reportName-keyed
//!   datacenter tables (no token, public)
//! - `np-anotice-stock.eastmoney.com/api/security/ann` — A-share
//!   announcement bulletin board (returns `art_code` → PDF URL pattern)
//! - `reportapi.eastmoney.com/report/list` — sell-side research
//!   indexes (returns `infoCode` → research PDF URL pattern)
//!
//! Common gotchas (from `docs/dispatches/M4.1-eastmoney-survey.md`):
//!
//! - `Referer: https://emweb.securities.eastmoney.com/` or
//!   `https://data.eastmoney.com/` keeps every endpoint stable.
//! - Filter values must use double quotes inside the `filter=` query
//!   string. URL-encoded as `%22…%22`.
//! - push2 `fs` parameters use `%20` as the OR delimiter.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::types::{
    AnnouncementItem, BlockTradeItem, Candle, HsgtFlow, LiveFlow, LiveQuote,
    ResearchReportItem, Symbol, VendorError,
};

const VENDOR: &str = "eastmoney";
const TIMEOUT_SECS: u64 = 12;
#[allow(dead_code)]
const REFERER_F10: &str = "https://emweb.securities.eastmoney.com/";
const REFERER_DATA: &str = "https://data.eastmoney.com/";
const REFERER_QUOTE: &str = "https://quote.eastmoney.com/";

pub struct EastmoneyClient {
    http: reqwest::Client,
}

impl EastmoneyClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

// ── push2 single quote + flow ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Push2Single {
    data: Option<Push2SingleData>,
}

#[derive(Debug, Deserialize)]
struct Push2SingleData(BTreeMap<String, Value>);

impl EastmoneyClient {
    /// `qt/stock/get` — push2 realtime quote for one symbol. Falls
    /// silent (returns Err::recoverable) outside trading hours since
    /// the upstream sometimes returns empty `data`.
    pub async fn push2_quote(&self, symbol: &Symbol) -> Result<LiveQuote, VendorError> {
        let secid = symbol.to_eastmoney();
        let resp = self
            .http
            .get("https://push2.eastmoney.com/api/qt/stock/get")
            .header("Referer", REFERER_QUOTE)
            .query(&[
                ("secid", secid.as_str()),
                ("fields", "f43,f44,f45,f46,f47,f48,f57,f58,f60,f86,f168,f169,f170,f171"),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("push2 quote HTTP failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("push2 quote HTTP {}", resp.status()),
            ));
        }
        let env: Push2Single = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("push2 quote JSON: {e}")))?;
        let Push2SingleData(m) = env
            .data
            .ok_or_else(|| VendorError::recoverable(VENDOR, "push2 quote: empty body"))?;
        // f43 现价 (×100), f168 换手率 (%), f169 涨跌额 ×100, f170 涨跌幅 ×100,
        // f47 成交量 (手), f48 成交额 (元), f57 secid copy, f58 name, f60 prev_close ×100,
        // f86 timestamp unix.
        let price = f64_div100(m.get("f43"));
        Ok(LiveQuote {
            symbol: symbol.to_dotted(),
            name: m.get("f58").and_then(|v| v.as_str()).map(String::from),
            price,
            change: f64_div100(m.get("f169")),
            change_pct: f64_div100(m.get("f170")),
            open: f64_div100(m.get("f46")),
            high: f64_div100(m.get("f44")),
            low: f64_div100(m.get("f45")),
            prev_close: f64_div100(m.get("f60")),
            volume_shares: m
                .get("f47")
                .and_then(|v| v.as_f64())
                .map(|v| v * 100.0),
            turnover_yuan: m.get("f48").and_then(|v| v.as_f64()),
            turnover_rate: f64_div100(m.get("f168")),
            timestamp_unix: m.get("f86").and_then(|v| v.as_i64()),
        })
    }

    /// `qt/ulist.np/get` — push2 batch realtime quote + capital flow.
    /// Returns one row per requested secid. Used by `market_pulse`.
    pub async fn push2_batch_quote_flow(
        &self,
        symbols: &[Symbol],
    ) -> Result<Vec<(LiveQuote, LiveFlow)>, VendorError> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        let secids = symbols
            .iter()
            .map(|s| s.to_eastmoney())
            .collect::<Vec<_>>()
            .join(",");
        // fields:
        // f12 code, f14 name, f2 last (×100), f3 chg pct (×100), f4 chg (×100),
        // f5 vol (手), f6 amt (元), f15/16/17/18 high/low/open/prev (×100),
        // f62/184 main net amt / pct, f66/72/78/84 super/large/medium/small net amt,
        // f124 timestamp unix.
        let resp = self
            .http
            .get("https://push2.eastmoney.com/api/qt/ulist.np/get")
            .header("Referer", REFERER_QUOTE)
            .query(&[
                ("secids", secids.as_str()),
                (
                    "fields",
                    "f1,f2,f3,f4,f5,f6,f12,f13,f14,f15,f16,f17,f18,f62,f184,f66,f72,f78,f84,f124",
                ),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| {
                VendorError::recoverable(VENDOR, format!("push2 batch HTTP failed: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("push2 batch HTTP {}", resp.status()),
            ));
        }
        let env: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("push2 batch JSON: {e}")))?;
        let rows = env
            .get("data")
            .and_then(|d| d.get("diff"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if rows.is_empty() {
            return Err(VendorError::recoverable(VENDOR, "push2 batch: empty diff"));
        }
        // Map by stock code so we can preserve the caller order. The
        // upstream sometimes reorders rows or returns f13 prefix (0/1)
        // separate from f12 code.
        let mut by_code: BTreeMap<String, &serde_json::Value> = BTreeMap::new();
        for row in &rows {
            if let Some(code) = row.get("f12").and_then(|v| v.as_str()) {
                by_code.insert(code.to_string(), row);
            }
        }
        let mut out = Vec::new();
        for sym in symbols {
            let Some(row) = by_code.get(&sym.code).copied() else {
                continue;
            };
            let quote = LiveQuote {
                symbol: sym.to_dotted(),
                name: row.get("f14").and_then(|v| v.as_str()).map(String::from),
                price: f64_div100(row.get("f2")),
                change_pct: f64_div100(row.get("f3")),
                change: f64_div100(row.get("f4")),
                volume_shares: row.get("f5").and_then(|v| v.as_f64()).map(|v| v * 100.0),
                turnover_yuan: row.get("f6").and_then(|v| v.as_f64()),
                open: f64_div100(row.get("f17")),
                high: f64_div100(row.get("f15")),
                low: f64_div100(row.get("f16")),
                prev_close: f64_div100(row.get("f18")),
                turnover_rate: None,
                timestamp_unix: row.get("f124").and_then(|v| v.as_i64()),
            };
            let flow = LiveFlow {
                symbol: sym.to_dotted(),
                main_net_yuan: row.get("f62").and_then(|v| v.as_f64()),
                main_net_pct: row.get("f184").and_then(|v| v.as_f64()),
                super_net_yuan: row.get("f66").and_then(|v| v.as_f64()),
                large_net_yuan: row.get("f72").and_then(|v| v.as_f64()),
                medium_net_yuan: row.get("f78").and_then(|v| v.as_f64()),
                small_net_yuan: row.get("f84").and_then(|v| v.as_f64()),
            };
            out.push((quote, flow));
        }
        Ok(out)
    }

    /// `push2his` daily K-line — kept as a same-day intraday fallback
    /// for `chart_data` (M4.2 will use this when intraday range goes
    /// live). Currently unused by the shipping tools; left in so the
    /// next milestone can wire it without touching the adapter file.
    #[allow(dead_code)]
    pub async fn push2his_kline(
        &self,
        symbol: &Symbol,
        period: &str,
        count: usize,
    ) -> Result<Vec<Candle>, VendorError> {
        let klt = match period {
            "1d" => "101",
            "1w" => "102",
            "1mo" => "103",
            other => {
                return Err(VendorError::fatal(
                    VENDOR,
                    format!("unsupported kline period '{other}' (try 1d/1w/1mo)"),
                ))
            }
        };
        let secid = symbol.to_eastmoney();
        let resp = self
            .http
            .get("https://push2his.eastmoney.com/api/qt/stock/kline/get")
            .header("Referer", REFERER_QUOTE)
            .query(&[
                ("secid", secid.as_str()),
                ("klt", klt),
                ("fqt", "1"),
                ("end", "20500101"),
                ("lmt", &count.to_string()),
                ("fields1", "f1,f2,f3,f4,f5,f6"),
                ("fields2", "f51,f52,f53,f54,f55,f56,f57,f58"),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("kline HTTP failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("kline HTTP {}", resp.status()),
            ));
        }
        let env: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("kline JSON: {e}")))?;
        let lines = env
            .get("data")
            .and_then(|d| d.get("klines"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let candles: Vec<Candle> = lines
            .iter()
            .filter_map(|v| v.as_str().and_then(parse_kline_row))
            .collect();
        if candles.is_empty() {
            return Err(VendorError::recoverable(VENDOR, "kline: empty klines"));
        }
        Ok(candles)
    }

    /// `qt/ulist.np/get` — push2 index quotes (sh000001 / sz399001 /
    /// sz399006 etc.). Used by `market_overview/snapshot`.
    pub async fn push2_indexes(
        &self,
        index_secids: &[&str],
    ) -> Result<Vec<LiveQuote>, VendorError> {
        let resp = self
            .http
            .get("https://push2.eastmoney.com/api/qt/ulist.np/get")
            .header("Referer", REFERER_QUOTE)
            .query(&[
                ("secids", index_secids.join(",").as_str()),
                (
                    "fields",
                    "f1,f2,f3,f4,f5,f6,f12,f14,f15,f16,f17,f18,f124",
                ),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| {
                VendorError::recoverable(VENDOR, format!("push2 idx HTTP failed: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("push2 idx HTTP {}", resp.status()),
            ));
        }
        let env: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("push2 idx JSON: {e}")))?;
        let rows = env
            .get("data")
            .and_then(|d| d.get("diff"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows
            .iter()
            .map(|row| LiveQuote {
                symbol: row
                    .get("f12")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_default(),
                name: row.get("f14").and_then(|v| v.as_str()).map(String::from),
                price: f64_div100(row.get("f2")),
                change_pct: f64_div100(row.get("f3")),
                change: f64_div100(row.get("f4")),
                volume_shares: row.get("f5").and_then(|v| v.as_f64()).map(|v| v * 100.0),
                turnover_yuan: row.get("f6").and_then(|v| v.as_f64()),
                open: f64_div100(row.get("f17")),
                high: f64_div100(row.get("f15")),
                low: f64_div100(row.get("f16")),
                prev_close: f64_div100(row.get("f18")),
                turnover_rate: None,
                timestamp_unix: row.get("f124").and_then(|v| v.as_i64()),
            })
            .collect())
    }
}

// ── datacenter-web reports ───────────────────────────────────────────

impl EastmoneyClient {
    /// Generic datacenter-web `/api/data/v1/get` helper. Returns the
    /// `result.data` array on success.
    pub(crate) async fn datacenter_rows(
        &self,
        report_name: &str,
        filter: Option<&str>,
        sort: Option<(&str, &str)>,
        page_size: usize,
    ) -> Result<Vec<serde_json::Value>, VendorError> {
        let page_size_str = page_size.to_string();
        let mut q: Vec<(&str, &str)> = vec![
            ("reportName", report_name),
            ("columns", "ALL"),
            ("pageSize", page_size_str.as_str()),
            ("pageNumber", "1"),
            ("source", "WEB"),
            ("client", "WEB"),
        ];
        if let Some(f) = filter {
            q.push(("filter", f));
        }
        if let Some((col, typ)) = sort {
            q.push(("sortColumns", col));
            q.push(("sortTypes", typ));
        }
        let resp = self
            .http
            .get("https://datacenter-web.eastmoney.com/api/data/v1/get")
            .header("Referer", REFERER_DATA)
            .query(&q)
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| {
                VendorError::recoverable(VENDOR, format!("dc {report_name} HTTP: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("dc {report_name} HTTP {}", resp.status()),
            ));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("dc {report_name} JSON: {e}")))?;
        Ok(body
            .get("result")
            .and_then(|r| r.get("data"))
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// `RPT_STOCKVALUATIONTANTILE` — historical valuation percentile
    /// snapshot. Returns `(pe_pct30, pe_pct50, pe_pct70, pb_pct50)`.
    pub async fn valuation_percentile(
        &self,
        symbol: &Symbol,
    ) -> Result<BTreeMap<String, Option<f64>>, VendorError> {
        let filter = format!(r#"(SECUCODE="{}.{}")(STATISTICS_CYCLE="3")"#, symbol.code, symbol.exchange);
        let rows = self
            .datacenter_rows("RPT_STOCKVALUATIONTANTILE", Some(&filter), None, 50)
            .await?;
        let mut out: BTreeMap<String, Option<f64>> = BTreeMap::new();
        for row in &rows {
            let idx_type = row.get("INDEX_TYPE").and_then(|v| v.as_str()).unwrap_or("");
            let label = match idx_type {
                "1" => "pe",
                "2" => "pb",
                other => other,
            };
            out.insert(
                format!("{label}_p30"),
                row.get("PERCENTILE_THIRTY").and_then(|v| v.as_f64()),
            );
            out.insert(
                format!("{label}_p50"),
                row.get("PERCENTILE_FIFTY").and_then(|v| v.as_f64()),
            );
            out.insert(
                format!("{label}_p70"),
                row.get("PERCENTILE_SEVENTY").and_then(|v| v.as_f64()),
            );
        }
        Ok(out)
    }

    /// `RPT_F10_EH_RELATION` — actual controller / 一致行动人.
    pub async fn actual_controller(
        &self,
        symbol: &Symbol,
    ) -> Result<Option<String>, VendorError> {
        let filter = format!(r#"(SECUCODE="{}.{}")"#, symbol.code, symbol.exchange);
        let rows = self
            .datacenter_rows("RPT_F10_EH_RELATION", Some(&filter), None, 20)
            .await?;
        Ok(rows
            .iter()
            .find_map(|row| {
                let rel = row.get("RELATED_RELATION").and_then(|v| v.as_str())?;
                if rel.contains("实际控制人") || rel.contains("控股股东") {
                    row.get("HOLDER_NAME")
                        .and_then(|v| v.as_str())
                        .map(|s| format!("{s}({rel})"))
                } else {
                    None
                }
            }))
    }

    /// `RPT_F10_MAIN_ORGHOLDDETAILS` — institutional holders (funds /
    /// QFII / 社保 / 保险). Returns rows as raw `BTreeMap` for the
    /// caller to project.
    pub async fn org_hold_details(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<serde_json::Value>, VendorError> {
        let filter = format!(r#"(SECUCODE="{}.{}")"#, symbol.code, symbol.exchange);
        self.datacenter_rows("RPT_F10_MAIN_ORGHOLDDETAILS", Some(&filter), None, 20)
            .await
    }

    /// Northbound persistent holding for one symbol (most recent).
    /// Kept available for future expansion of `stock_overview.holders`
    /// to surface 北向 quarterly持股. Not wired in M4.1.1 — surfaces as
    /// dead-code until then.
    #[allow(dead_code)]
    pub async fn north_hold(&self, symbol: &Symbol) -> Result<Option<serde_json::Value>, VendorError> {
        let filter = format!(r#"(SECUCODE="{}.{}")"#, symbol.code, symbol.exchange);
        let rows = self
            .datacenter_rows(
                "RPT_MUTUAL_HOLDRANK_NEW",
                Some(&filter),
                Some(("HOLD_DATE", "-1")),
                10,
            )
            .await?;
        Ok(rows.into_iter().next())
    }

    /// 块交易 (datacenter alternative) — used as a richer block-trade
    /// surface when the tushare path returns empty. Currently not
    /// exposed; placeholder for future.
    #[allow(dead_code)]
    pub async fn block_trade_dc(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<BlockTradeItem>, VendorError> {
        let filter = format!(r#"(SECUCODE="{}.{}")"#, symbol.code, symbol.exchange);
        let rows = self
            .datacenter_rows(
                "RPT_DATA_BLOCKTRADE",
                Some(&filter),
                Some(("TRADE_DATE", "-1")),
                30,
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| BlockTradeItem {
                trade_date: r
                    .get("TRADE_DATE")
                    .and_then(|v| v.as_str())
                    .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
                    .unwrap_or_default(),
                price: r.get("DEAL_PRICE").and_then(|v| v.as_f64()),
                vol_wan_shares: r.get("DEAL_VOLUME").and_then(|v| v.as_f64()),
                amount_wan_yuan: r.get("DEAL_AMT").and_then(|v| v.as_f64()),
                buyer: r.get("BUYER_NAME").and_then(|v| v.as_str()).map(String::from),
                seller: r
                    .get("SELLER_NAME")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            })
            .collect())
    }
}

// ── Announcement bulletin + research index ───────────────────────────

impl EastmoneyClient {
    /// `np-anotice-stock/api/security/ann` — full announcement bulletin
    /// for one symbol over a window (the upstream pageSize is 50 — we
    /// keep one page and post-filter to the user's `days`).
    pub async fn announcements(
        &self,
        symbol: &Symbol,
        days: usize,
    ) -> Result<Vec<AnnouncementItem>, VendorError> {
        let stock_code = symbol.code.clone();
        let resp = self
            .http
            .get("https://np-anotice-stock.eastmoney.com/api/security/ann")
            .header("Referer", REFERER_DATA)
            .query(&[
                ("sr", "-1"),
                ("page_size", "50"),
                ("page_index", "1"),
                ("ann_type", "A"),
                ("client_source", "web"),
                ("stock_list", stock_code.as_str()),
                ("f_node", "0"),
                ("s_node", "0"),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| {
                VendorError::recoverable(VENDOR, format!("ann HTTP failed: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("ann HTTP {}", resp.status()),
            ));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("ann JSON: {e}")))?;
        let rows = body
            .get("data")
            .and_then(|d| d.get("list"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let cutoff = chrono::Utc::now().naive_utc().date() - chrono::Duration::days(days as i64);
        Ok(rows
            .iter()
            .filter_map(|row| {
                let title = row.get("title").and_then(|v| v.as_str())?.to_string();
                let raw_date = row
                    .get("notice_date")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let date_only = raw_date
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if let Ok(d) = chrono::NaiveDate::parse_from_str(&date_only, "%Y-%m-%d") {
                    if d < cutoff {
                        return None;
                    }
                }
                let art_code = row.get("art_code").and_then(|v| v.as_str());
                let pdf_url = art_code.map(|c| format!("https://pdf.dfcfw.com/pdf/H2_{c}_1.pdf"));
                Some(AnnouncementItem {
                    date: date_only,
                    category: Some(super::tushare::classify_ann_title(&title)),
                    title,
                    url: art_code.map(|c| {
                        format!(
                            "https://np-cnotice-stock.eastmoney.com/api/content/ann?art_code={c}"
                        )
                    }),
                    pdf_url,
                })
            })
            .collect())
    }

    /// `reportapi.eastmoney.com/report/list` — listing of broker
    /// research reports for one symbol with the `infoCode` field that
    /// maps to a `H3_*` PDF URL.
    pub async fn research_reports(
        &self,
        symbol: &Symbol,
        begin: &str,
        end: &str,
        page_size: usize,
    ) -> Result<Vec<ResearchReportItem>, VendorError> {
        let page_size_str = page_size.to_string();
        let resp = self
            .http
            .get("https://reportapi.eastmoney.com/report/list")
            .header("Referer", REFERER_DATA)
            .query(&[
                ("qType", "0"),
                ("code", symbol.code.as_str()),
                ("beginTime", begin),
                ("endTime", end),
                ("pageSize", page_size_str.as_str()),
                ("pageNo", "1"),
                ("industryCode", "*"),
                ("industry", "*"),
                ("rating", "*"),
                ("ratingChange", "*"),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| {
                VendorError::recoverable(VENDOR, format!("research list HTTP: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("research list HTTP {}", resp.status()),
            ));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("research list JSON: {e}")))?;
        let rows = body
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(rows
            .iter()
            .filter_map(|row| {
                let title = row.get("title").and_then(|v| v.as_str())?.to_string();
                let date = row
                    .get("publishDate")
                    .and_then(|v| v.as_str())
                    .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
                    .unwrap_or_default();
                let info_code = row.get("infoCode").and_then(|v| v.as_str());
                let pdf_url =
                    info_code.map(|c| format!("https://pdf.dfcfw.com/pdf/H3_{c}_1.pdf"));
                Some(ResearchReportItem {
                    date,
                    title,
                    org_name: row
                        .get("orgSName")
                        .or_else(|| row.get("orgName"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    author: row.get("researcher").and_then(|v| v.as_str()).map(String::from),
                    rating: row.get("emRatingName").and_then(|v| v.as_str()).map(String::from),
                    target_price: row.get("indvAimPriceT").and_then(|v| match v {
                        serde_json::Value::Number(n) => n.as_f64(),
                        serde_json::Value::String(s) => s.parse().ok(),
                        _ => None,
                    }),
                    pdf_url,
                })
            })
            .collect())
    }

    /// `kamt/get` — total northbound + southbound dayNetAmt snapshot.
    /// Returns the southbound side (hk2sh + hk2sz) which is still live;
    /// northbound from this endpoint has been zero-padded since 2024-08-19.
    pub async fn kamt(&self) -> Result<Option<HsgtFlow>, VendorError> {
        let resp = self
            .http
            .get("https://push2.eastmoney.com/api/qt/kamt/get")
            .header("Referer", REFERER_QUOTE)
            .query(&[
                ("fields1", "f1,f2,f3,f4"),
                ("fields2", "f51,f52,f54,f56"),
                ("ut", "b2884a393a59ad64002292a3e90d46a5"),
            ])
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("kamt HTTP: {e}")))?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("kamt HTTP {}", resp.status()),
            ));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VendorError::recoverable(VENDOR, format!("kamt JSON: {e}")))?;
        let south = body
            .get("data")
            .and_then(|d| d.get("s2n"))
            .and_then(|v| v.get("dayNetAmtIn"))
            .and_then(|v| v.as_f64());
        let north = body
            .get("data")
            .and_then(|d| d.get("hk2sh"))
            .and_then(|v| v.get("dayNetAmtIn"))
            .and_then(|v| v.as_f64());
        Ok(Some(HsgtFlow {
            trade_date: chrono::Utc::now()
                .naive_utc()
                .date()
                .format("%Y-%m-%d")
                .to_string(),
            north_money_wan: north,
            south_money_wan: south,
            hgt_wan: None,
            sgt_wan: None,
        }))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// EastMoney push2 returns most prices as `value × 100`. Convert back.
fn f64_div100(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| match x {
        Value::Number(n) => n.as_f64().map(|v| v / 100.0),
        Value::String(s) => s.parse::<f64>().ok().map(|v| v / 100.0),
        _ => None,
    })
}

/// Parse one EastMoney kline row: `date,open,close,high,low,vol,turnover,amplitude%,…`.
/// Used by `push2his_kline` (dead-code today; live in M4.2 intraday).
#[allow(dead_code)]
pub(crate) fn parse_kline_row(raw: &str) -> Option<Candle> {
    let fields: Vec<&str> = raw.split(',').collect();
    if fields.len() < 7 {
        return None;
    }
    let date = fields[0].to_string();
    let open: f64 = fields[1].parse().ok()?;
    let close: f64 = fields[2].parse().ok()?;
    let high: f64 = fields[3].parse().ok()?;
    let low: f64 = fields[4].parse().ok()?;
    let volume: f64 = fields[5].parse().unwrap_or(0.0) * 100.0; // 手 → 股
    let turnover: f64 = fields[6].parse().unwrap_or(0.0); // 元
    Some(Candle {
        date,
        open,
        high,
        low,
        close,
        volume,
        turnover: Some(turnover),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_kline_row() {
        let row = "2026-05-20,1810.00,1825.30,1830.00,1805.10,24567,4485000000,1.38";
        let c = parse_kline_row(row).unwrap();
        assert_eq!(c.date, "2026-05-20");
        assert!((c.open - 1810.0).abs() < 1e-6);
        assert!((c.close - 1825.30).abs() < 1e-6);
        assert!((c.volume - 2_456_700.0).abs() < 1.0);
        assert!((c.turnover.unwrap() - 4_485_000_000.0).abs() < 1.0);
    }

    #[test]
    fn truncated_row_returns_none() {
        assert!(parse_kline_row("only,three,fields").is_none());
        assert!(parse_kline_row("").is_none());
    }

    #[test]
    fn malformed_number_returns_none() {
        let row = "2026-05-20,notnum,1825.30,1830.00,1805.10,24567,4485000000";
        assert!(parse_kline_row(row).is_none());
    }

    #[test]
    fn f64_div100_converts_string_or_number() {
        assert_eq!(
            f64_div100(Some(&Value::Number(serde_json::Number::from(12738)))),
            Some(127.38)
        );
        assert_eq!(
            f64_div100(Some(&Value::String("12738".into()))),
            Some(127.38)
        );
        assert_eq!(f64_div100(None), None);
        assert_eq!(f64_div100(Some(&Value::Null)), None);
    }
}
