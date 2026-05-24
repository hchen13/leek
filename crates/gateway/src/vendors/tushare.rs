//! Tushare client — the primary A-share data vendor (M3).
//!
//! Tushare exposes one POST endpoint that switches on `api_name`:
//!
//! ```text
//! POST http://api.tushare.pro
//! { "api_name": "daily", "token": "...", "params": { "ts_code": "600519.SH" }, "fields": "..." }
//! ```
//!
//! Responses come back as a column-oriented matrix:
//!
//! ```json
//! { "code": 0, "msg": null, "data": { "fields": [...], "items": [[...], ...] } }
//! ```
//!
//! Every vendor-specific Tushare field name (`or_tot`, `n_income_attr_p`,
//! `pe_ttm`, …) is mapped INSIDE this file to vendor-neutral keys
//! (`revenue`, `net_profit`, `pe_ttm` — that last one happens to be
//! generic too). The grep guardrail at `tests/m3-vendor-neutrality.rs`
//! ensures no Tushare schema name leaks past this module.
//!
//! Token resolution: `LEEK_TUSHARE_TOKEN` env > `Config::tushare_token` >
//! none. With no token the client errors recoverably so the dispatch
//! path moves on to Sina / EastMoney.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::types::{CapitalFlow, Candle, CompanyInfo, FinancialReport, FinancialRow, MarketQuote, Symbol};
use super::vendor_trait::{
    BoxFuture, VendorCandle, VendorCapitalFlow, VendorCompanyInfo, VendorError, VendorFinancial,
    VendorQuote,
};

const VENDOR: &str = "tushare";
const TUSHARE_URL: &str = "http://api.tushare.pro";
const TIMEOUT_SECS: u64 = 20;

/// Tushare HTTP client. `token=None` makes every fetch return a
/// recoverable error tagged `"no token"` — the dispatch path moves on.
pub struct TushareClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl TushareClient {
    pub fn new(http: reqwest::Client, token: Option<String>) -> Self {
        Self { http, token }
    }

    /// Tushare response envelope. `data` is the columnar matrix.
    fn token_or_error(&self) -> Result<&str, VendorError> {
        self.token
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                VendorError::recoverable(
                    VENDOR,
                    "no token configured (set LEEK_TUSHARE_TOKEN or tushare_token in ~/.leek/config.json)",
                )
            })
    }

    async fn post(
        &self,
        api_name: &str,
        params: Value,
        fields: &str,
    ) -> Result<TushareResponse, VendorError> {
        let token = self.token_or_error()?;
        let body = serde_json::json!({
            "api_name": api_name,
            "token": token,
            "params": params,
            "fields": fields,
        });
        tracing::debug!(vendor = VENDOR, api = api_name, "tushare request");
        let resp = self
            .http
            .post(TUSHARE_URL)
            .json(&body)
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| {
                VendorError::recoverable(VENDOR, format!("{api_name} HTTP failed: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("{api_name} HTTP {}", resp.status()),
            ));
        }
        let parsed: TushareResponse = resp.json().await.map_err(|e| {
            VendorError::recoverable(VENDOR, format!("{api_name} JSON parse failed: {e}"))
        })?;
        if parsed.code != 0 {
            // 40001 = bad token, 40002 = quota, etc. All recoverable.
            return Err(VendorError::recoverable(
                VENDOR,
                format!(
                    "{api_name} returned code={} msg={}",
                    parsed.code,
                    parsed.msg.as_deref().unwrap_or("(none)")
                ),
            ));
        }
        Ok(parsed)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct TushareResponse {
    pub code: i64,
    pub msg: Option<String>,
    pub data: Option<TushareData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TushareData {
    pub fields: Vec<String>,
    pub items: Vec<Vec<Value>>,
}

impl TushareData {
    /// One row of the matrix → `BTreeMap<field, value>` for ergonomic
    /// extraction by name.
    pub fn row_map(&self, row: &[Value]) -> BTreeMap<String, Value> {
        self.fields
            .iter()
            .zip(row.iter())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// ── Field extraction helpers ──────────────────────────────────────────

fn f64_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `20260520` → `2026-05-20`. Tushare's compact date format.
fn iso_date(raw: &str) -> String {
    if raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..8])
    } else {
        raw.into()
    }
}

// ── VendorQuote ───────────────────────────────────────────────────────

impl VendorQuote for TushareClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_quote<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<MarketQuote, VendorError>> {
        Box::pin(async move {
            // `daily` is end-of-day OHLCV — Tushare's free tier doesn't
            // expose realtime. Take the latest row.
            let resp = self
                .post(
                    "daily",
                    serde_json::json!({ "ts_code": symbol.to_dotted() }),
                    "ts_code,trade_date,open,high,low,close,pre_close,change,pct_chg,vol,amount",
                )
                .await?;
            let data = resp.data.ok_or_else(|| {
                VendorError::recoverable(VENDOR, "daily: empty data envelope")
            })?;
            let row = data.items.first().ok_or_else(|| {
                VendorError::recoverable(VENDOR, "daily: no rows for symbol")
            })?;
            let m = data.row_map(row);
            let close = f64_of(m.get("close").unwrap_or(&Value::Null))
                .ok_or_else(|| VendorError::recoverable(VENDOR, "daily: missing close"))?;
            let change = f64_of(m.get("change").unwrap_or(&Value::Null)).unwrap_or(0.0);
            let pct = f64_of(m.get("pct_chg").unwrap_or(&Value::Null)).unwrap_or(0.0);
            let trade_date = str_of(m.get("trade_date").unwrap_or(&Value::Null))
                .unwrap_or_else(|| "".into());
            Ok(MarketQuote {
                symbol: symbol.to_dotted(),
                name: None, // daily endpoint doesn't return name; merged downstream
                price: close,
                change,
                change_pct: pct,
                volume: f64_of(m.get("vol").unwrap_or(&Value::Null)).map(|v| v * 100.0), // 手 → 股
                turnover: f64_of(m.get("amount").unwrap_or(&Value::Null)).map(|v| v * 1000.0), // 千元 → 元
                open: f64_of(m.get("open").unwrap_or(&Value::Null)),
                high: f64_of(m.get("high").unwrap_or(&Value::Null)),
                low: f64_of(m.get("low").unwrap_or(&Value::Null)),
                prev_close: f64_of(m.get("pre_close").unwrap_or(&Value::Null)),
                timestamp: iso_date(&trade_date),
                data_freshness: "close".into(),
            })
        })
    }
}

// ── VendorCandle ──────────────────────────────────────────────────────

impl VendorCandle for TushareClient {
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
            let api = match period {
                "1d" => "daily",
                "1w" => "weekly",
                "1mo" => "monthly",
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!("unsupported period '{other}' (try 1d/1w/1mo)"),
                    ))
                }
            };
            let resp = self
                .post(
                    api,
                    serde_json::json!({ "ts_code": symbol.to_dotted() }),
                    "ts_code,trade_date,open,high,low,close,vol,amount",
                )
                .await?;
            let data = resp.data.ok_or_else(|| {
                VendorError::recoverable(VENDOR, format!("{api}: empty data envelope"))
            })?;
            // Tushare returns newest-first; we want a chronological list
            // capped at `count`.
            let mut candles: Vec<Candle> = data
                .items
                .iter()
                .take(count)
                .filter_map(|row| {
                    let m = data.row_map(row);
                    let date = str_of(m.get("trade_date").unwrap_or(&Value::Null))?;
                    Some(Candle {
                        date: iso_date(&date),
                        open: f64_of(m.get("open").unwrap_or(&Value::Null))?,
                        high: f64_of(m.get("high").unwrap_or(&Value::Null))?,
                        low: f64_of(m.get("low").unwrap_or(&Value::Null))?,
                        close: f64_of(m.get("close").unwrap_or(&Value::Null))?,
                        volume: f64_of(m.get("vol").unwrap_or(&Value::Null))
                            .map(|v| v * 100.0)
                            .unwrap_or(0.0),
                        turnover: f64_of(m.get("amount").unwrap_or(&Value::Null))
                            .map(|v| v * 1000.0),
                    })
                })
                .collect();
            candles.reverse();
            if candles.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("{api}: no candles for symbol"),
                ));
            }
            Ok(candles)
        })
    }
}

// ── VendorFinancial ───────────────────────────────────────────────────

impl VendorFinancial for TushareClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_financials<'a>(
        &'a self,
        symbol: &'a Symbol,
        statement: &'a str,
        period: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<FinancialReport, VendorError>> {
        Box::pin(async move {
            // Each statement maps to a different Tushare endpoint with a
            // distinct field surface. We translate vendor-specific names
            // to the neutral metric keys promised in `FinancialRow`.
            let (api, fields, key_map): (
                &str,
                &str,
                &[(&str, &str)],
            ) = match statement {
                "income" => (
                    "income",
                    "ts_code,end_date,total_revenue,revenue,oper_cost,operate_profit,total_profit,n_income,n_income_attr_p,basic_eps",
                    &[
                        ("revenue", "total_revenue"),
                        ("operating_revenue", "revenue"),
                        ("operating_cost", "oper_cost"),
                        ("operating_profit", "operate_profit"),
                        ("total_profit", "total_profit"),
                        ("net_profit", "n_income"),
                        ("net_profit_to_parent", "n_income_attr_p"),
                        ("eps_basic", "basic_eps"),
                    ],
                ),
                "balance" => (
                    "balancesheet",
                    "ts_code,end_date,total_assets,total_liab,total_hldr_eqy_inc_min_int,money_cap,total_cur_assets,total_cur_liab",
                    &[
                        ("total_assets", "total_assets"),
                        ("total_liabilities", "total_liab"),
                        ("total_equity", "total_hldr_eqy_inc_min_int"),
                        ("cash_and_equivalents", "money_cap"),
                        ("current_assets", "total_cur_assets"),
                        ("current_liabilities", "total_cur_liab"),
                    ],
                ),
                "cashflow" => (
                    "cashflow",
                    "ts_code,end_date,n_cashflow_act,n_cashflow_inv_act,n_cash_flows_fnc_act,free_cashflow",
                    &[
                        ("cf_operating", "n_cashflow_act"),
                        ("cf_investing", "n_cashflow_inv_act"),
                        ("cf_financing", "n_cash_flows_fnc_act"),
                        ("free_cash_flow", "free_cashflow"),
                    ],
                ),
                "ratios" => (
                    "fina_indicator",
                    "ts_code,end_date,roe,roa,grossprofit_margin,netprofit_margin,debt_to_assets,current_ratio,quick_ratio",
                    &[
                        ("roe", "roe"),
                        ("roa", "roa"),
                        ("gross_margin", "grossprofit_margin"),
                        ("net_margin", "netprofit_margin"),
                        // tushare gives debt-to-assets; we expose as
                        // debt_to_equity-like ratio (note semantic
                        // difference is documented downstream).
                        ("debt_to_assets", "debt_to_assets"),
                        ("current_ratio", "current_ratio"),
                        ("quick_ratio", "quick_ratio"),
                    ],
                ),
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!(
                            "unsupported statement '{other}' (try income/balance/cashflow/ratios)"
                        ),
                    ));
                }
            };

            let params = if period == "year" {
                serde_json::json!({
                    "ts_code": symbol.to_dotted(),
                    "period": "1231",
                })
            } else {
                serde_json::json!({ "ts_code": symbol.to_dotted() })
            };

            let resp = self.post(api, params, fields).await?;
            let data = resp.data.ok_or_else(|| {
                VendorError::recoverable(VENDOR, format!("{api}: empty data envelope"))
            })?;

            // Filter to annual rows if `period == "year"` — Tushare's
            // returns include all quarters; annual ones end on 1231.
            let filter_annual = period == "year";
            let rows: Vec<FinancialRow> = data
                .items
                .iter()
                .filter_map(|row| {
                    let m = data.row_map(row);
                    let end = str_of(m.get("end_date").unwrap_or(&Value::Null))?;
                    if filter_annual && !end.ends_with("1231") {
                        return None;
                    }
                    let mut metrics = BTreeMap::new();
                    for (neutral, tushare_key) in key_map {
                        let v = m.get(*tushare_key).cloned().unwrap_or(Value::Null);
                        metrics.insert((*neutral).into(), f64_of(&v));
                    }
                    Some(FinancialRow {
                        period_end: iso_date(&end),
                        label: derive_label(&end, period),
                        metrics,
                    })
                })
                .take(count)
                .collect();

            if rows.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("{api}: no rows for symbol/period"),
                ));
            }
            Ok(FinancialReport {
                symbol: symbol.to_dotted(),
                statement: statement.into(),
                period: period.into(),
                rows,
            })
        })
    }
}

fn derive_label(end_date: &str, period: &str) -> String {
    // `20241231` → `2024` (annual) or `2024Q4` (quarter).
    if end_date.len() != 8 {
        return end_date.into();
    }
    let year = &end_date[..4];
    if period == "year" {
        return year.into();
    }
    let month = &end_date[4..6];
    let q = match month {
        "03" => "Q1",
        "06" => "Q2",
        "09" => "Q3",
        "12" => "Q4",
        _ => "",
    };
    format!("{year}{q}")
}

// ── VendorCompanyInfo ─────────────────────────────────────────────────

impl VendorCompanyInfo for TushareClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_company_info<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<CompanyInfo, VendorError>> {
        Box::pin(async move {
            // stock_basic: name, industry, list_date, exchange
            let basic = self
                .post(
                    "stock_basic",
                    serde_json::json!({ "ts_code": symbol.to_dotted() }),
                    "ts_code,name,area,industry,fullname,exchange,list_date",
                )
                .await?;
            let basic_data = basic.data.ok_or_else(|| {
                VendorError::recoverable(VENDOR, "stock_basic: empty data envelope")
            })?;
            let basic_row = basic_data.items.first().ok_or_else(|| {
                VendorError::recoverable(VENDOR, "stock_basic: no rows")
            })?;
            let bm = basic_data.row_map(basic_row);

            // daily_basic: market cap + P/E + P/B + dividend yield (latest).
            // Best-effort — if it fails we still return the basic profile.
            let mut indicators = BTreeMap::new();
            match self
                .post(
                    "daily_basic",
                    serde_json::json!({ "ts_code": symbol.to_dotted() }),
                    "ts_code,trade_date,total_mv,circ_mv,pe_ttm,pb,dv_ttm",
                )
                .await
            {
                Ok(db) => {
                    if let Some(d) = db.data {
                        if let Some(row) = d.items.first() {
                            let dm = d.row_map(row);
                            // Tushare returns in 万元 — convert to 元.
                            indicators.insert(
                                "market_cap".into(),
                                f64_of(dm.get("total_mv").unwrap_or(&Value::Null))
                                    .map(|v| v * 10_000.0),
                            );
                            indicators.insert(
                                "float_market_cap".into(),
                                f64_of(dm.get("circ_mv").unwrap_or(&Value::Null))
                                    .map(|v| v * 10_000.0),
                            );
                            indicators.insert(
                                "pe_ttm".into(),
                                f64_of(dm.get("pe_ttm").unwrap_or(&Value::Null)),
                            );
                            indicators.insert(
                                "pb".into(),
                                f64_of(dm.get("pb").unwrap_or(&Value::Null)),
                            );
                            indicators.insert(
                                "dividend_yield_pct".into(),
                                f64_of(dm.get("dv_ttm").unwrap_or(&Value::Null)),
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        vendor = VENDOR,
                        symbol = %symbol.to_dotted(),
                        error = %e.message,
                        "daily_basic enrichment failed; returning bare profile",
                    );
                }
            }

            let exchange_label = str_of(bm.get("exchange").unwrap_or(&Value::Null)).map(|raw| {
                match raw.as_str() {
                    "SSE" => "上海证券交易所".into(),
                    "SZSE" => "深圳证券交易所".into(),
                    "BSE" => "北京证券交易所".into(),
                    _ => raw,
                }
            });

            Ok(CompanyInfo {
                symbol: symbol.to_dotted(),
                name: str_of(bm.get("name").unwrap_or(&Value::Null))
                    .unwrap_or_default(),
                industry: str_of(bm.get("industry").unwrap_or(&Value::Null)),
                sector: str_of(bm.get("area").unwrap_or(&Value::Null)),
                exchange: exchange_label,
                list_date: str_of(bm.get("list_date").unwrap_or(&Value::Null))
                    .map(|s| iso_date(&s)),
                business_scope: None, // stock_company endpoint; not all free quotas include
                latest_indicators: indicators,
            })
        })
    }
}

// ── VendorCapitalFlow ─────────────────────────────────────────────────

impl VendorCapitalFlow for TushareClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }
    fn fetch_capital_flow<'a>(
        &'a self,
        symbol: &'a Symbol,
        period: &'a str,
    ) -> BoxFuture<'a, Result<CapitalFlow, VendorError>> {
        Box::pin(async move {
            let window = match period {
                "1d" => 1usize,
                "5d" => 5,
                "20d" => 20,
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!("unsupported capital-flow period '{other}' (try 1d/5d/20d)"),
                    ))
                }
            };
            // `moneyflow`: per-day buy/sell broken into trade sizes.
            // Tushare lumps `xl` (extra large) + `lg` (large) as the
            // "main force"; `md` (medium) + `sm` (small) as retail.
            let resp = self
                .post(
                    "moneyflow",
                    serde_json::json!({ "ts_code": symbol.to_dotted() }),
                    "ts_code,trade_date,buy_lg_amount,sell_lg_amount,buy_md_amount,sell_md_amount,buy_sm_amount,sell_sm_amount,buy_elg_amount,sell_elg_amount,net_mf_amount",
                )
                .await?;
            let data = resp.data.ok_or_else(|| {
                VendorError::recoverable(VENDOR, "moneyflow: empty data envelope")
            })?;
            let rows: Vec<_> = data.items.iter().take(window).collect();
            if rows.is_empty() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    "moneyflow: no rows for symbol",
                ));
            }
            // Tushare amounts are in 万元 — convert to 元.
            let to_yuan = |v: Option<f64>| v.map(|n| n * 10_000.0);
            let mut main_net = 0.0f64;
            let mut retail_net = 0.0f64;
            let mut total_net = 0.0f64;
            for row in &rows {
                let m = data.row_map(row);
                let main = (to_yuan(f64_of(m.get("buy_elg_amount").unwrap_or(&Value::Null)))
                    .unwrap_or(0.0)
                    - to_yuan(f64_of(m.get("sell_elg_amount").unwrap_or(&Value::Null)))
                        .unwrap_or(0.0))
                    + (to_yuan(f64_of(m.get("buy_lg_amount").unwrap_or(&Value::Null)))
                        .unwrap_or(0.0)
                        - to_yuan(f64_of(m.get("sell_lg_amount").unwrap_or(&Value::Null)))
                            .unwrap_or(0.0));
                let retail = (to_yuan(f64_of(m.get("buy_md_amount").unwrap_or(&Value::Null)))
                    .unwrap_or(0.0)
                    - to_yuan(f64_of(m.get("sell_md_amount").unwrap_or(&Value::Null)))
                        .unwrap_or(0.0))
                    + (to_yuan(f64_of(m.get("buy_sm_amount").unwrap_or(&Value::Null)))
                        .unwrap_or(0.0)
                        - to_yuan(f64_of(m.get("sell_sm_amount").unwrap_or(&Value::Null)))
                            .unwrap_or(0.0));
                let net = to_yuan(f64_of(m.get("net_mf_amount").unwrap_or(&Value::Null)))
                    .unwrap_or(main + retail);
                main_net += main;
                retail_net += retail;
                total_net += net;
            }

            // Northbound (`hsgt_top10` is per-day per-stock from Stock
            // Connect). Most free tokens don't have access; treat any
            // error as "unavailable" rather than fatal.
            let (north_avail, north_flow, mut warnings) = self
                .try_north_flow(symbol, window)
                .await
                .map(|v| (true, Some(v), vec![]))
                .unwrap_or_else(|e| {
                    (false, None, vec![format!("northbound flow unavailable: {}", e.message)])
                });
            if window > 1 {
                warnings.push(format!(
                    "period={period} aggregates {window} trading days; rolling totals."
                ));
            }
            Ok(CapitalFlow {
                symbol: symbol.to_dotted(),
                period: period.into(),
                net_inflow: total_net,
                main_flow: Some(main_net),
                retail_flow: Some(retail_net),
                north_flow_available: north_avail,
                north_flow,
                warnings,
            })
        })
    }
}

impl TushareClient {
    /// Best-effort northbound aggregation. `hsgt_top10` requires a
    /// higher quota than the free tier and is frequently denied — any
    /// error here is treated as "unavailable" by the caller.
    async fn try_north_flow(
        &self,
        symbol: &Symbol,
        window: usize,
    ) -> Result<f64, VendorError> {
        let resp = self
            .post(
                "hsgt_top10",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,trade_date,net_amount",
            )
            .await?;
        let data = resp
            .data
            .ok_or_else(|| VendorError::recoverable(VENDOR, "hsgt_top10: empty"))?;
        let sum: f64 = data
            .items
            .iter()
            .take(window)
            .filter_map(|row| {
                let m = data.row_map(row);
                f64_of(m.get("net_amount").unwrap_or(&Value::Null)).map(|v| v * 10_000.0)
            })
            .sum();
        Ok(sum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_response(fields: Vec<&str>, items: Vec<Vec<Value>>) -> TushareResponse {
        TushareResponse {
            code: 0,
            msg: None,
            data: Some(TushareData {
                fields: fields.into_iter().map(String::from).collect(),
                items,
            }),
        }
    }

    #[test]
    fn no_token_yields_recoverable_error() {
        let c = TushareClient::new(reqwest::Client::new(), None);
        let err = c.token_or_error().unwrap_err();
        assert!(err.recoverable);
        assert!(err.message.contains("LEEK_TUSHARE_TOKEN"));
    }

    #[test]
    fn iso_date_normalizes_compact_form() {
        assert_eq!(iso_date("20260520"), "2026-05-20");
        assert_eq!(iso_date("2026-05-20"), "2026-05-20"); // pass-through
    }

    #[test]
    fn row_map_zips_fields_and_values() {
        let data = fake_response(
            vec!["a", "b", "c"],
            vec![vec![Value::Number(1.into()), Value::String("x".into()), Value::Null]],
        )
        .data
        .unwrap();
        let m = data.row_map(&data.items[0]);
        assert_eq!(m.get("a"), Some(&Value::Number(1.into())));
        assert_eq!(m.get("c"), Some(&Value::Null));
    }

    #[test]
    fn derive_label_quarterly_and_annual() {
        assert_eq!(derive_label("20241231", "year"), "2024");
        assert_eq!(derive_label("20240331", "quarter"), "2024Q1");
        assert_eq!(derive_label("20240630", "quarter"), "2024Q2");
        assert_eq!(derive_label("20240930", "quarter"), "2024Q3");
        assert_eq!(derive_label("20241231", "quarter"), "2024Q4");
    }

    #[test]
    fn f64_of_parses_number_and_string() {
        assert_eq!(f64_of(&Value::Number(serde_json::Number::from(42))), Some(42.0));
        // `1825.30` (茅台's order of magnitude, not a math constant) so
        // clippy doesn't flag `3.14` as approximating PI.
        assert_eq!(f64_of(&Value::String("1825.30".into())), Some(1825.30));
        assert_eq!(f64_of(&Value::Null), None);
    }
}
