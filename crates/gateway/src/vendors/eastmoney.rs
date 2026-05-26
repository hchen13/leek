//! EastMoney push2his — A-share candlestick fallback (M3).
//!
//! Endpoint: `https://push2his.eastmoney.com/api/qt/stock/kline/get`. Required
//! query params:
//!
//! | param  | meaning                                                |
//! | ------ | ------------------------------------------------------ |
//! | secid  | `1.<code>` (SH) / `0.<code>` (SZ)                      |
//! | klt    | 101 = day, 102 = week, 103 = month                     |
//! | fqt    | 1 = forward-adjust (we want this for split alignment)  |
//! | end    | `20500101` (a sentinel "max")                          |
//! | lmt    | row count                                              |
//! | fields1| `f1,f2,f3,f4,f5,f6` (metadata)                         |
//! | fields2| `f51,f52,f53,f54,f55,f56,f57,f58` (date,o,c,h,l,v,a)   |
//!
//! Response shape:
//!
//! ```json
//! { "data": { "klines": ["2026-05-20,1810,1825,1830,1805,2456789,4485000000,...", ...] } }
//! ```
//!
//! Comma-separated string per row. EastMoney only implements
//! `VendorCandle`. Other shapes fall through to the next vendor.

use serde::Deserialize;

use super::types::{
    AnalystConsensus, AnnouncementItem, AnnouncementList, BusinessBreakdown,
    BusinessBreakdownRow, Candle, ConceptList, ConceptTag, ConsensusForecast,
    ConsensusRatingMix, HolderRow, Symbol, TopHolders,
};
use super::vendor_trait::{
    BoxFuture, VendorAnnouncements, VendorBusinessBreakdown, VendorCandle, VendorConcepts,
    VendorConsensus, VendorError, VendorTopHolders,
};

const VENDOR: &str = "eastmoney";
const EASTMONEY_KLINE_URL: &str = "https://push2his.eastmoney.com/api/qt/stock/kline/get";
/// Datacenter `get` endpoint — most F10 supplementary boards use it.
const EASTMONEY_DATACENTER_URL: &str = "https://datacenter-web.eastmoney.com/api/data/v1/get";
/// Announcement bulletin board.
const EASTMONEY_ANN_URL: &str = "https://np-anotice-stock.eastmoney.com/api/security/ann";
const TIMEOUT_SECS: u64 = 10;
/// Most F10 endpoints reject requests without an EastMoney `Referer`.
const REFERER: &str = "https://emweb.securities.eastmoney.com/";

pub struct EastmoneyClient {
    http: reqwest::Client,
}

impl EastmoneyClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[derive(Debug, Deserialize)]
struct KlineEnvelope {
    data: Option<KlineData>,
}

#[derive(Debug, Deserialize)]
struct KlineData {
    klines: Option<Vec<String>>,
}

impl VendorCandle for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }

    fn fetch_candles<'a>(
        &'a self,
        symbol: &'a Symbol,
        period: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<Vec<Candle>, VendorError>> {
        Box::pin(async move {
            let klt = match period {
                "1d" => "101",
                "1w" => "102",
                "1mo" => "103",
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!("unsupported period '{other}' (try 1d/1w/1mo)"),
                    ));
                }
            };
            let secid = symbol.to_eastmoney();
            let resp = self
                .http
                .get(EASTMONEY_KLINE_URL)
                .query(&[
                    ("secid", secid.as_str()),
                    ("klt", klt),
                    ("fqt", "1"), // forward-adjusted prices
                    ("end", "20500101"),
                    ("lmt", &count.to_string()),
                    ("fields1", "f1,f2,f3,f4,f5,f6"),
                    ("fields2", "f51,f52,f53,f54,f55,f56,f57,f58"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| VendorError::recoverable(VENDOR, format!("HTTP failed: {e}")))?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("HTTP {}", resp.status()),
                ));
            }
            let env: KlineEnvelope = resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("JSON parse failed: {e}"))
            })?;
            let klines = env
                .data
                .and_then(|d| d.klines)
                .ok_or_else(|| VendorError::recoverable(VENDOR, "empty klines envelope"))?;
            let candles: Vec<Candle> = klines
                .iter()
                .filter_map(|s| parse_kline_row(s))
                .collect();
            if candles.is_empty() {
                return Err(VendorError::recoverable(VENDOR, "no candles in response"));
            }
            Ok(candles)
        })
    }
}

/// Parse one EastMoney kline row: `date,open,close,high,low,volume,turnover,amplitude%[,…]`.
/// Public-in-crate for unit tests.
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
    let turnover: f64 = fields[6].parse().unwrap_or(0.0); // already 元
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

// ── M4.1 EastMoney fallbacks ─────────────────────────────────────────
//
// EastMoney's F10 boards back onto a small set of stable JSON endpoints
// (`emweb.securities.eastmoney.com` and `datacenter-web.eastmoney.com`).
// All return `{ result: {...} | data: [...] }`. We hit those directly
// rather than scraping HTML so that page-layout changes don't break us.
// The endpoints used here are public — no token, no rate-limit beyond
// polite use.

impl VendorAnnouncements for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_announcements<'a>(
        &'a self,
        symbol: &'a Symbol,
        days: usize,
        category: Option<&'a str>,
    ) -> BoxFuture<'a, Result<AnnouncementList, VendorError>> {
        Box::pin(async move {
            // The bulletin endpoint paginates by page_size & page_index.
            // We pull one page (50 rows) covering recent history, then
            // post-filter by days.
            let stock_code = &symbol.code;
            let resp = self
                .http
                .get(EASTMONEY_ANN_URL)
                .header("Referer", REFERER)
                .query(&[
                    ("page_size", "50"),
                    ("page_index", "1"),
                    ("ann_type", "A"), // A-share
                    ("client_source", "web"),
                    ("stock_list", stock_code.as_str()),
                    ("f_node", "0"),
                    ("s_node", "0"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| VendorError::recoverable(VENDOR, format!("ann HTTP failed: {e}")))?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("ann HTTP {}", resp.status()),
                ));
            }
            let body: serde_json::Value = resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("ann JSON parse failed: {e}"))
            })?;
            let rows = body
                .get("data")
                .and_then(|d| d.get("list"))
                .and_then(|l| l.as_array())
                .cloned()
                .unwrap_or_default();
            let cutoff =
                chrono::Utc::now().naive_utc().date() - chrono::Duration::days(days as i64);
            let mut items: Vec<AnnouncementItem> = rows
                .iter()
                .filter_map(|row| {
                    let title = row
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())?;
                    // EastMoney uses `notice_date` formatted as `YYYY-MM-DD HH:MM:SS`.
                    let date_full = row
                        .get("notice_date")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let date_only = date_full.split_whitespace().next().unwrap_or("").to_string();
                    // Window filter (`date_only` may be empty for malformed rows).
                    if let Ok(d) = chrono::NaiveDate::parse_from_str(&date_only, "%Y-%m-%d") {
                        if d < cutoff {
                            return None;
                        }
                    }
                    let art_code = row.get("art_code").and_then(|v| v.as_str());
                    let url = art_code.map(|c| {
                        format!("https://np-cnotice-stock.eastmoney.com/api/content/ann?art_code={c}")
                    });
                    Some(AnnouncementItem {
                        date: date_only,
                        title: title.clone(),
                        category: Some(super::tushare::classify_announcement(&title)),
                        url,
                        abstract_text: None,
                    })
                })
                .collect();
            let mut seen = std::collections::HashSet::new();
            items.retain(|a| seen.insert(a.title.clone()));
            if let Some(cat) = category {
                items.retain(|a| {
                    a.category
                        .as_deref()
                        .map(|c| c.contains(cat))
                        .unwrap_or(false)
                });
            }
            if items.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "ann: no announcements matched window/filter",
                ));
            }
            Ok(AnnouncementList {
                symbol: symbol.to_dotted(),
                days,
                category_filter: category.map(|s| s.to_string()),
                items,
            })
        })
    }
}

impl VendorBusinessBreakdown for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_business_breakdown<'a>(
        &'a self,
        symbol: &'a Symbol,
        _period: &'a str,
        dim: &'a str,
    ) -> BoxFuture<'a, Result<BusinessBreakdown, VendorError>> {
        Box::pin(async move {
            // `RPT_F10_FN_GZJCFXBQ` is EastMoney's main-business breakdown
            // table; the `RPTTYPE` column distinguishes product / industry
            // / region.
            let rpttype = match dim {
                "product" => "1",
                "industry" => "2",
                "region" => "3",
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!(
                            "unsupported breakdown dim '{other}' (try product/industry/region)"
                        ),
                    ))
                }
            };
            let filter = format!(
                "(SECUCODE=\"{}.{}\")(RPTTYPE={})",
                symbol.code, symbol.exchange, rpttype
            );
            let resp = self
                .http
                .get(EASTMONEY_DATACENTER_URL)
                .header("Referer", REFERER)
                .query(&[
                    ("reportName", "RPT_F10_FN_GZJCFXBQ"),
                    ("columns", "ALL"),
                    ("filter", filter.as_str()),
                    ("pageSize", "50"),
                    ("pageNumber", "1"),
                    ("sortColumns", "REPORT_DATE,RANK"),
                    ("sortTypes", "-1,1"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| {
                    VendorError::recoverable(VENDOR, format!("breakdown HTTP failed: {e}"))
                })?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("breakdown HTTP {}", resp.status()),
                ));
            }
            let body: serde_json::Value = resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("breakdown JSON parse failed: {e}"))
            })?;
            let rows = body
                .get("result")
                .and_then(|r| r.get("data"))
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "breakdown: empty result set",
                ));
            }
            // Most-recent REPORT_DATE wins.
            let latest = rows
                .iter()
                .filter_map(|r| r.get("REPORT_DATE").and_then(|v| v.as_str()).map(String::from))
                .max()
                .unwrap_or_default();
            let rows: Vec<_> = rows
                .iter()
                .filter(|r| {
                    r.get("REPORT_DATE")
                        .and_then(|v| v.as_str())
                        .map(|s| s == latest)
                        .unwrap_or(false)
                })
                .collect();
            // Total for ratio computation (EastMoney sometimes precomputes
            // RANK=0 as 总计 — we use the sum across non-总计 rows).
            let total: f64 = rows
                .iter()
                .filter(|r| {
                    r.get("ITEM_NAME")
                        .and_then(|v| v.as_str())
                        .map(|s| !s.contains("合计") && !s.contains("总计"))
                        .unwrap_or(true)
                })
                .filter_map(|r| r.get("MAIN_BUSINESS_INCOME").and_then(|v| v.as_f64()))
                .sum();
            let items: Vec<BusinessBreakdownRow> = rows
                .iter()
                .filter(|r| {
                    r.get("ITEM_NAME")
                        .and_then(|v| v.as_str())
                        .map(|s| !s.contains("合计") && !s.contains("总计"))
                        .unwrap_or(true)
                })
                .map(|r| {
                    let revenue = r.get("MAIN_BUSINESS_INCOME").and_then(|v| v.as_f64());
                    let pct = revenue.and_then(|s| if total > 0.0 { Some(s / total * 100.0) } else { None });
                    BusinessBreakdownRow {
                        item: r
                            .get("ITEM_NAME")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(未知)")
                            .to_string(),
                        revenue_yuan: revenue,
                        pct_of_total: pct,
                        gross_margin_pct: r.get("GROSS_RPOFIT_RATIO").and_then(|v| v.as_f64()),
                        yoy_change_pct: r
                            .get("MBI_RATIO")
                            .and_then(|v| v.as_f64()),
                    }
                })
                .collect();
            // Normalize the period to ISO `YYYY-MM-DD` (EastMoney sends
            // `2025-06-30 00:00:00`).
            let period_end = latest.split_whitespace().next().unwrap_or("").to_string();
            Ok(BusinessBreakdown {
                symbol: symbol.to_dotted(),
                period_end,
                dimension: dim.to_string(),
                items,
            })
        })
    }
}

impl VendorTopHolders for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_top_holders<'a>(
        &'a self,
        symbol: &'a Symbol,
        kind: &'a str,
    ) -> BoxFuture<'a, Result<TopHolders, VendorError>> {
        Box::pin(async move {
            let report_name = match kind {
                "total" => "RPT_F10_EH_HOLDERSNEW",
                "float" => "RPT_F10_EH_FREEHOLDERS",
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!("unsupported holders kind '{other}' (try total/float)"),
                    ))
                }
            };
            let filter = format!("(SECUCODE=\"{}.{}\")", symbol.code, symbol.exchange);
            let resp = self
                .http
                .get(EASTMONEY_DATACENTER_URL)
                .header("Referer", REFERER)
                .query(&[
                    ("reportName", report_name),
                    ("columns", "ALL"),
                    ("filter", filter.as_str()),
                    ("pageSize", "10"),
                    ("pageNumber", "1"),
                    ("sortColumns", "END_DATE,HOLDER_RANK"),
                    ("sortTypes", "-1,1"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| {
                    VendorError::recoverable(VENDOR, format!("holders HTTP failed: {e}"))
                })?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("holders HTTP {}", resp.status()),
                ));
            }
            let body: serde_json::Value = resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("holders JSON parse failed: {e}"))
            })?;
            let rows = body
                .get("result")
                .and_then(|r| r.get("data"))
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "holders: empty result set",
                ));
            }
            let latest = rows
                .iter()
                .filter_map(|r| r.get("END_DATE").and_then(|v| v.as_str()).map(String::from))
                .max()
                .unwrap_or_default();
            let holders: Vec<HolderRow> = rows
                .iter()
                .filter(|r| {
                    r.get("END_DATE")
                        .and_then(|v| v.as_str())
                        .map(|s| s == latest)
                        .unwrap_or(false)
                })
                .enumerate()
                .map(|(idx, r)| HolderRow {
                    rank: r
                        .get("HOLDER_RANK")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32)
                        .unwrap_or((idx + 1) as u32),
                    holder_name: r
                        .get("HOLDER_NAME")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(未知)")
                        .to_string(),
                    shares: r.get("HOLD_NUM").and_then(|v| v.as_f64()),
                    pct: r.get("HOLD_NUM_RATIO").and_then(|v| v.as_f64()),
                    change_qoq_shares: r.get("HOLD_NUM_CHANGE").and_then(|v| v.as_f64()),
                })
                .take(10)
                .collect();
            Ok(TopHolders {
                symbol: symbol.to_dotted(),
                kind: kind.to_string(),
                period_end: latest.split_whitespace().next().unwrap_or("").to_string(),
                holders,
            })
        })
    }
}

impl VendorConcepts for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_concepts<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<ConceptList, VendorError>> {
        Box::pin(async move {
            // RPT_F10_CORETHEME_BOARDTYPE returns concept memberships
            // for a symbol — sorted by board popularity when available.
            let filter = format!("(SECUCODE=\"{}.{}\")", symbol.code, symbol.exchange);
            let resp = self
                .http
                .get(EASTMONEY_DATACENTER_URL)
                .header("Referer", REFERER)
                .query(&[
                    ("reportName", "RPT_F10_CORETHEME_BOARDTYPE"),
                    ("columns", "ALL"),
                    ("filter", filter.as_str()),
                    ("pageSize", "50"),
                    ("pageNumber", "1"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| {
                    VendorError::recoverable(VENDOR, format!("concepts HTTP failed: {e}"))
                })?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("concepts HTTP {}", resp.status()),
                ));
            }
            let body: serde_json::Value = resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("concepts JSON parse failed: {e}"))
            })?;
            let rows = body
                .get("result")
                .and_then(|r| r.get("data"))
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "concepts: empty result set",
                ));
            }
            // BOARD_TYPE: 1=industry, 2=concept (题材). Spec asks for concepts.
            // Filter and cap at 30.
            let concepts: Vec<ConceptTag> = rows
                .iter()
                .filter(|r| {
                    r.get("BOARD_TYPE")
                        .and_then(|v| v.as_str())
                        .map(|s| s == "概念板块" || s.contains("概念"))
                        .unwrap_or(true) // be permissive — if unsure, include
                })
                .filter_map(|r| {
                    let name = r
                        .get("BOARD_NAME")
                        .and_then(|v| v.as_str())
                        .map(String::from)?;
                    let code = r
                        .get("BOARD_CODE")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    Some(ConceptTag {
                        name,
                        code,
                        heat_rank: None,
                    })
                })
                .take(30)
                .collect();
            if concepts.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "concepts: no concept tags after filtering",
                ));
            }
            Ok(ConceptList {
                symbol: symbol.to_dotted(),
                concepts,
            })
        })
    }
}

impl VendorConsensus for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_consensus<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<AnalystConsensus, VendorError>> {
        Box::pin(async move {
            // Two endpoints feed consensus: forecasts + ratings.
            // RPT_RES_FORECAST_AGRG aggregates EPS / net-profit means
            // per fiscal year. RPT_RES_PROFITPRIDX gives rating counts.
            let filter = format!("(SECURITY_CODE=\"{}\")", symbol.code);
            let forecasts_resp = self
                .http
                .get(EASTMONEY_DATACENTER_URL)
                .header("Referer", REFERER)
                .query(&[
                    ("reportName", "RPT_RES_FORECAST_AGRG"),
                    ("columns", "ALL"),
                    ("filter", filter.as_str()),
                    ("pageSize", "5"),
                    ("pageNumber", "1"),
                    ("sortColumns", "PREDICT_YEAR"),
                    ("sortTypes", "1"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| {
                    VendorError::recoverable(VENDOR, format!("consensus HTTP failed: {e}"))
                })?;
            if !forecasts_resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("consensus HTTP {}", forecasts_resp.status()),
                ));
            }
            let f_body: serde_json::Value = forecasts_resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("consensus JSON parse failed: {e}"))
            })?;
            let rows = f_body
                .get("result")
                .and_then(|r| r.get("data"))
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            let forecasts: Vec<ConsensusForecast> = rows
                .iter()
                .filter_map(|r| {
                    let year_raw =
                        r.get("PREDICT_YEAR").or_else(|| r.get("FORECAST_YEAR"))?;
                    let year = match year_raw {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => return None,
                    };
                    // Net-profit aggregates in EastMoney are 亿元.
                    let np_mean = r
                        .get("NET_PROFIT_AVG")
                        .or_else(|| r.get("NET_PROFIT_PREDICT_AVG"))
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1.0e8);
                    let np_high = r
                        .get("NET_PROFIT_HIGH")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1.0e8);
                    let np_low = r
                        .get("NET_PROFIT_LOW")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1.0e8);
                    let rev_mean = r
                        .get("OPERATE_INCOME_AVG")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1.0e8);
                    let rev_high = r
                        .get("OPERATE_INCOME_HIGH")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1.0e8);
                    let rev_low = r
                        .get("OPERATE_INCOME_LOW")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1.0e8);
                    let eps_mean = r.get("EPS_AVG").and_then(|v| v.as_f64());
                    let eps_high = r.get("EPS_HIGH").and_then(|v| v.as_f64());
                    let eps_low = r.get("EPS_LOW").and_then(|v| v.as_f64());
                    Some(ConsensusForecast {
                        year,
                        revenue_mean_yuan: rev_mean,
                        revenue_high_yuan: rev_high,
                        revenue_low_yuan: rev_low,
                        net_profit_mean_yuan: np_mean,
                        net_profit_high_yuan: np_high,
                        net_profit_low_yuan: np_low,
                        eps_mean,
                        eps_high,
                        eps_low,
                    })
                })
                .collect();
            if forecasts.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "consensus: no forecast rows",
                ));
            }
            // Ratings — best-effort. If this second call fails we still
            // surface forecasts with an empty rating mix.
            let ratings_resp = self
                .http
                .get(EASTMONEY_DATACENTER_URL)
                .header("Referer", REFERER)
                .query(&[
                    ("reportName", "RPT_RES_RATINGSTATIS"),
                    ("columns", "ALL"),
                    ("filter", filter.as_str()),
                    ("pageSize", "20"),
                    ("pageNumber", "1"),
                    ("sortColumns", "RATING_DATE"),
                    ("sortTypes", "-1"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await;
            let mut rating_mix = ConsensusRatingMix::default();
            let mut report_count = None;
            if let Ok(resp) = ratings_resp {
                if resp.status().is_success() {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(rows) = body
                            .get("result")
                            .and_then(|r| r.get("data"))
                            .and_then(|d| d.as_array())
                        {
                            for r in rows {
                                let rating = r
                                    .get("RATING_NAME")
                                    .or_else(|| r.get("RATING_CHANGE"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default();
                                super::tushare::bump_rating(&mut rating_mix, rating);
                            }
                            report_count = Some(rows.len());
                        }
                    }
                }
            }
            Ok(AnalystConsensus {
                symbol: symbol.to_dotted(),
                forecasts,
                rating_mix,
                report_count,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_kline_row() {
        // Real EastMoney row format.
        let row = "2026-05-20,1810.00,1825.30,1830.00,1805.10,24567,4485000000,1.38";
        let c = parse_kline_row(row).unwrap();
        assert_eq!(c.date, "2026-05-20");
        assert!((c.open - 1810.00).abs() < 1e-6);
        assert!((c.close - 1825.30).abs() < 1e-6);
        assert!((c.high - 1830.00).abs() < 1e-6);
        assert!((c.low - 1805.10).abs() < 1e-6);
        // 24567 手 → 2,456,700 股
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
}
