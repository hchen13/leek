//! Tushare HTTP adapter (M4.1.1 facts-only).
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
//! Every vendor-specific Tushare field name (`or_tot`, `n_income_attr_p`, …)
//! is mapped INSIDE this module to vendor-neutral keys (`revenue`,
//! `net_profit`, …). The grep guardrail at `tests/m3_vendor_neutrality.rs`
//! ensures no Tushare schema name leaks past this module.
//!
//! Token resolution lives in `vendors/mod.rs`: env > config > none. With
//! no token the client errors recoverably so tools surface "this dimension
//! is empty" instead of fabricating.
//!
//! ## Method discipline
//!
//! Each public method below is a one-call wrapper around a tushare
//! endpoint that returns a typed shape. **Fan-out happens inside the
//! tool**, never here — that lets tools join concurrently with
//! `tokio::try_join!` and surface partial gaps cleanly.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::types::{
    AnnouncementItem, BlockTradeItem, BrokerMonthlyPick, Candle, CompanyProfile, ConceptConstituent,
    ConceptItem, CpiPpiObs, DividendItem, FinancialRow, GdpObs, HolderCountObs, HolderRow,
    HsgtFlow, IndexBasic, IndustryFlowRow, InsiderTradeItem, InstitutionVisitItem, LimitCapt,
    LimitListRow, LiveQuote, MacroEventItem, MarketTotals, MoneyObs, PledgeStatItem, PmiObs,
    PolicyItem, RatingMix, RepurchaseItem, ResearchReportItem, ShareUnlockItem, ShiborLprObs,
    SocialFinancingObs, Symbol, TechRow, TopListItem, UsTbrObs, VendorError, YearForecast,
};

const VENDOR: &str = "tushare";
const TUSHARE_URL: &str = "http://api.tushare.pro";
const TIMEOUT_SECS: u64 = 20;

/// Tushare HTTP client. `token=None` makes every fetch return a
/// recoverable error tagged `"no token"`.
pub struct TushareClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl TushareClient {
    pub fn new(http: reqwest::Client, token: Option<String>) -> Self {
        Self { http, token }
    }

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

    /// Generic POST wrapper. Surfaces tushare's `code != 0` as a
    /// recoverable error so callers can pivot.
    pub(crate) async fn post(
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
            .map_err(|e| VendorError::recoverable(VENDOR, format!("{api_name} HTTP failed: {e}")))?;
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
    pub fn row_map(&self, row: &[Value]) -> BTreeMap<String, Value> {
        self.fields
            .iter()
            .zip(row.iter())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// ── Field extraction helpers ──────────────────────────────────────────

pub(crate) fn f64_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

pub(crate) fn str_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `20260520` → `2026-05-20`. Tushare's compact date form.
pub(crate) fn iso_date(raw: &str) -> String {
    if raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..8])
    } else {
        raw.into()
    }
}

fn rows_take<'a>(data: &'a TushareData, n: usize) -> impl Iterator<Item = BTreeMap<String, Value>> + 'a {
    data.items.iter().take(n).map(|r| data.row_map(r))
}

// ── A. Macro indicators (`macro_indicators` tool) ────────────────────

impl TushareClient {
    /// `cn_cpi` — newest first.
    pub async fn cn_cpi(&self, months: usize) -> Result<Vec<CpiPpiObs>, VendorError> {
        let resp = self
            .post(
                "cn_cpi",
                serde_json::json!({}),
                "month,nt_val,nt_yoy,nt_mom,nt_accu",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "cn_cpi: empty data envelope")
        })?;
        Ok(rows_take(&data, months)
            .map(|m| CpiPpiObs {
                month: str_of(m.get("month").unwrap_or(&Value::Null)).unwrap_or_default(),
                nation_value: f64_of(m.get("nt_val").unwrap_or(&Value::Null)),
                nation_yoy: f64_of(m.get("nt_yoy").unwrap_or(&Value::Null)),
                nation_mom: f64_of(m.get("nt_mom").unwrap_or(&Value::Null)),
                nation_accum_yoy: f64_of(m.get("nt_accu").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `cn_ppi` — newest first.
    pub async fn cn_ppi(&self, months: usize) -> Result<Vec<CpiPpiObs>, VendorError> {
        let resp = self
            .post(
                "cn_ppi",
                serde_json::json!({}),
                "month,ppi_yoy,ppi_mp,ppi_mp_qm",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "cn_ppi: empty data envelope")
        })?;
        Ok(rows_take(&data, months)
            .map(|m| CpiPpiObs {
                month: str_of(m.get("month").unwrap_or(&Value::Null)).unwrap_or_default(),
                nation_value: None,
                nation_yoy: f64_of(m.get("ppi_yoy").unwrap_or(&Value::Null)),
                nation_mom: f64_of(m.get("ppi_mp").unwrap_or(&Value::Null)),
                nation_accum_yoy: f64_of(m.get("ppi_mp_qm").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `cn_gdp` — newest first.
    pub async fn cn_gdp(&self, quarters: usize) -> Result<Vec<GdpObs>, VendorError> {
        let resp = self
            .post(
                "cn_gdp",
                serde_json::json!({}),
                "quarter,gdp,gdp_yoy,pi_yoy,si_yoy,ti_yoy",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "cn_gdp: empty data envelope")
        })?;
        Ok(rows_take(&data, quarters)
            .map(|m| GdpObs {
                quarter: str_of(m.get("quarter").unwrap_or(&Value::Null)).unwrap_or_default(),
                gdp_yi_yuan: f64_of(m.get("gdp").unwrap_or(&Value::Null)),
                gdp_yoy: f64_of(m.get("gdp_yoy").unwrap_or(&Value::Null)),
                primary_yoy: f64_of(m.get("pi_yoy").unwrap_or(&Value::Null)),
                secondary_yoy: f64_of(m.get("si_yoy").unwrap_or(&Value::Null)),
                tertiary_yoy: f64_of(m.get("ti_yoy").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `cn_pmi` — newest first. We only project the two headline series.
    pub async fn cn_pmi(&self, months: usize) -> Result<Vec<PmiObs>, VendorError> {
        // The vendor returns ~60 PMI sub-codes; pull all and project.
        let resp = self
            .post("cn_pmi", serde_json::json!({}), "")
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "cn_pmi: empty data envelope")
        })?;
        Ok(rows_take(&data, months)
            .map(|m| PmiObs {
                month: str_of(m.get("MONTH").or_else(|| m.get("month")).unwrap_or(&Value::Null))
                    .unwrap_or_default(),
                manufacturing: f64_of(m.get("PMI010000").unwrap_or(&Value::Null)),
                non_manufacturing: f64_of(m.get("PMI020100").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `cn_m` — money supply, newest first.
    pub async fn cn_m(&self, months: usize) -> Result<Vec<MoneyObs>, VendorError> {
        let resp = self
            .post(
                "cn_m",
                serde_json::json!({}),
                "month,m0,m1,m2,m0_yoy,m1_yoy,m2_yoy",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "cn_m: empty data envelope")
        })?;
        Ok(rows_take(&data, months)
            .map(|m| MoneyObs {
                month: str_of(m.get("month").unwrap_or(&Value::Null)).unwrap_or_default(),
                m0_yi_yuan: f64_of(m.get("m0").unwrap_or(&Value::Null)),
                m1_yi_yuan: f64_of(m.get("m1").unwrap_or(&Value::Null)),
                m2_yi_yuan: f64_of(m.get("m2").unwrap_or(&Value::Null)),
                m0_yoy: f64_of(m.get("m0_yoy").unwrap_or(&Value::Null)),
                m1_yoy: f64_of(m.get("m1_yoy").unwrap_or(&Value::Null)),
                m2_yoy: f64_of(m.get("m2_yoy").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `sf_month` — social financing monthly increment, newest first.
    pub async fn sf_month(&self, months: usize) -> Result<Vec<SocialFinancingObs>, VendorError> {
        let resp = self
            .post(
                "sf_month",
                serde_json::json!({}),
                "month,inc_month,inc_cumval,stk_endval",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "sf_month: empty data envelope")
        })?;
        Ok(rows_take(&data, months)
            .map(|m| SocialFinancingObs {
                month: str_of(m.get("month").unwrap_or(&Value::Null)).unwrap_or_default(),
                increment_yi_yuan: f64_of(m.get("inc_month").unwrap_or(&Value::Null)),
                cumulative_yi_yuan: f64_of(m.get("inc_cumval").unwrap_or(&Value::Null)),
                stock_wan_yi_yuan: f64_of(m.get("stk_endval").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `shibor_lpr` — newest first.
    pub async fn shibor_lpr(&self, rows: usize) -> Result<Vec<ShiborLprObs>, VendorError> {
        let resp = self
            .post("shibor_lpr", serde_json::json!({}), "date,1y,5y")
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "shibor_lpr: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| ShiborLprObs {
                date: iso_date(
                    &str_of(m.get("date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                lpr_1y: f64_of(m.get("1y").unwrap_or(&Value::Null)),
                lpr_5y: f64_of(m.get("5y").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `us_tbr` — newest first.
    pub async fn us_tbr(&self, rows: usize) -> Result<Vec<UsTbrObs>, VendorError> {
        let resp = self
            .post(
                "us_tbr",
                serde_json::json!({}),
                "date,w4_bd,w13_bd,w26_bd,w52_bd",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "us_tbr: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| UsTbrObs {
                date: iso_date(
                    &str_of(m.get("date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                w4_bd: f64_of(m.get("w4_bd").unwrap_or(&Value::Null)),
                w13_bd: f64_of(m.get("w13_bd").unwrap_or(&Value::Null)),
                w52_bd: f64_of(m.get("w52_bd").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `npr` — State Council policy library (国务院政策文件库).
    ///
    /// Tushare returns rows ordered newest-first. The `pubtime` field
    /// carries a full datetime (`2026-05-22 17:00:00`); we trim to the
    /// date portion for `PolicyItem.publish_date`. A `start_date` /
    /// `end_date` filter (`YYYYMMDD`) is honored by upstream when both
    /// are supplied.
    pub async fn npr(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PolicyItem>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(s) = start_date {
            params["start_date"] = s.into();
        }
        if let Some(e) = end_date {
            params["end_date"] = e.into();
        }
        let resp = self
            .post("npr", params, "pubtime,title,pcode,puborg,ptype")
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "npr: empty data envelope")
        })?;
        Ok(rows_take(&data, limit)
            .map(|m| {
                let raw_time =
                    str_of(m.get("pubtime").unwrap_or(&Value::Null)).unwrap_or_default();
                // `2026-05-22 17:00:00` → `2026-05-22`. Already-iso
                // dates pass through unchanged.
                let publish_date = raw_time
                    .split_whitespace()
                    .next()
                    .unwrap_or(&raw_time)
                    .to_string();
                PolicyItem {
                    publish_date,
                    title: str_of(m.get("title").unwrap_or(&Value::Null)).unwrap_or_default(),
                    publish_org: str_of(m.get("puborg").unwrap_or(&Value::Null)),
                    policy_id: str_of(m.get("pcode").unwrap_or(&Value::Null)),
                    category: str_of(m.get("ptype").unwrap_or(&Value::Null)),
                }
            })
            .collect())
    }

    /// `cn_schedule` — Chinese macro data release calendar.
    ///
    /// Note (M4.1.2): upstream currently ignores `start_date` / `end_date`
    /// filters under our token tier and returns the oldest entries first.
    /// We pass the filters through anyway (so any future fix is picked
    /// up automatically), then post-filter on the date window and trim
    /// to `limit`. Callers should sort the returned vec by date if a
    /// specific ordering matters.
    pub async fn cn_schedule(
        &self,
        start_date: Option<&str>,
        end_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MacroEventItem>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(s) = start_date {
            params["start_date"] = s.into();
        }
        if let Some(e) = end_date {
            params["end_date"] = e.into();
        }
        let resp = self
            .post(
                "cn_schedule",
                params,
                "month,publish_date,title,issuing_org,data_api",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "cn_schedule: empty data envelope")
        })?;
        // Pull ALL rows first because upstream may ignore the window
        // filter; we'll narrow client-side.
        let mut all: Vec<MacroEventItem> = data
            .items
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                let pub_raw =
                    str_of(m.get("publish_date").unwrap_or(&Value::Null)).unwrap_or_default();
                MacroEventItem {
                    publish_date: iso_date(&pub_raw),
                    title: str_of(m.get("title").unwrap_or(&Value::Null)).unwrap_or_default(),
                    issuing_org: str_of(m.get("issuing_org").unwrap_or(&Value::Null)),
                    period: str_of(m.get("month").unwrap_or(&Value::Null)),
                    data_api: str_of(m.get("data_api").unwrap_or(&Value::Null)),
                }
            })
            .collect();
        // Client-side window filter when the caller supplied bounds.
        if let (Some(s), Some(e)) = (start_date, end_date) {
            let s = iso_date(s);
            let e = iso_date(e);
            all.retain(|r| r.publish_date.as_str() >= s.as_str()
                && r.publish_date.as_str() <= e.as_str());
        }
        // Newest first inside the window.
        all.sort_by(|a, b| b.publish_date.cmp(&a.publish_date));
        all.truncate(limit);
        Ok(all)
    }
}

// ── B. Market overview helpers ───────────────────────────────────────

impl TushareClient {
    /// `daily_info` — per-market totals (沪 A / 沪 B / 深 A / …).
    pub async fn daily_info(
        &self,
        trade_date: Option<&str>,
    ) -> Result<Vec<MarketTotals>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(d) = trade_date {
            params["trade_date"] = d.into();
        }
        let resp = self
            .post(
                "daily_info",
                params,
                "trade_date,ts_code,ts_name,com_count,total_mv,amount,pe,tr,exchange",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "daily_info: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                MarketTotals {
                    ts_code: str_of(m.get("ts_code").unwrap_or(&Value::Null)).unwrap_or_default(),
                    ts_name: str_of(m.get("ts_name").unwrap_or(&Value::Null)).unwrap_or_default(),
                    com_count: f64_of(m.get("com_count").unwrap_or(&Value::Null)),
                    total_mv_yi_yuan: f64_of(m.get("total_mv").unwrap_or(&Value::Null)),
                    amount_yi_yuan: f64_of(m.get("amount").unwrap_or(&Value::Null)),
                    pe: f64_of(m.get("pe").unwrap_or(&Value::Null)),
                    turnover_rate: f64_of(m.get("tr").unwrap_or(&Value::Null)),
                    trade_date: iso_date(
                        &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                    ),
                }
            })
            .collect())
    }

    /// `index_dailybasic` — latest basic stats for the given index codes.
    pub async fn index_dailybasic(
        &self,
        ts_codes: &[&str],
        trade_date: Option<&str>,
    ) -> Result<Vec<IndexBasic>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(d) = trade_date {
            params["trade_date"] = d.into();
        }
        let resp = self
            .post(
                "index_dailybasic",
                params,
                "ts_code,trade_date,total_mv,pe,pe_ttm,pb,turnover_rate",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "index_dailybasic: empty data envelope")
        })?;
        let want: std::collections::HashSet<&&str> = ts_codes.iter().collect();
        Ok(data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                let ts =
                    str_of(m.get("ts_code").unwrap_or(&Value::Null)).unwrap_or_default();
                if !ts_codes.is_empty() && !want.contains(&ts.as_str()) {
                    return None;
                }
                Some(IndexBasic {
                    ts_code: ts,
                    trade_date: iso_date(
                        &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                    ),
                    total_mv_yuan: f64_of(m.get("total_mv").unwrap_or(&Value::Null))
                        .map(|v| v * 10_000.0),
                    pe: f64_of(m.get("pe").unwrap_or(&Value::Null)),
                    pe_ttm: f64_of(m.get("pe_ttm").unwrap_or(&Value::Null)),
                    pb: f64_of(m.get("pb").unwrap_or(&Value::Null)),
                    turnover_rate: f64_of(m.get("turnover_rate").unwrap_or(&Value::Null)),
                })
            })
            .collect())
    }

    /// `moneyflow_mkt_dc` — single-row big-picture market money flow.
    pub async fn moneyflow_mkt_dc(
        &self,
        trade_date: Option<&str>,
    ) -> Result<Option<BTreeMap<String, Option<f64>>>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(d) = trade_date {
            params["trade_date"] = d.into();
        }
        let resp = self.post("moneyflow_mkt_dc", params, "").await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "moneyflow_mkt_dc: empty data envelope")
        })?;
        let Some(row) = data.items.first() else { return Ok(None) };
        let m = data.row_map(row);
        let mut out = BTreeMap::new();
        for k in [
            "close_sh",
            "pct_change_sh",
            "close_sz",
            "pct_change_sz",
            "net_amount",
            "net_amount_rate",
            "buy_elg_amount",
            "buy_lg_amount",
            "buy_md_amount",
            "buy_sm_amount",
        ] {
            out.insert(k.into(), f64_of(m.get(k).unwrap_or(&Value::Null)));
        }
        out.insert(
            "trade_date".into(),
            f64_of(m.get("trade_date").unwrap_or(&Value::Null)),
        );
        Ok(Some(out))
    }

    /// `moneyflow_hsgt` — sh/sz/hk/total connect flow snapshot.
    pub async fn moneyflow_hsgt(
        &self,
        trade_date: Option<&str>,
    ) -> Result<Option<HsgtFlow>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(d) = trade_date {
            params["trade_date"] = d.into();
        }
        let resp = self
            .post(
                "moneyflow_hsgt",
                params,
                "trade_date,ggt_ss,ggt_sz,hgt,sgt,north_money,south_money",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "moneyflow_hsgt: empty data envelope")
        })?;
        let Some(row) = data.items.first() else { return Ok(None) };
        let m = data.row_map(row);
        Ok(Some(HsgtFlow {
            trade_date: iso_date(
                &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
            ),
            north_money_wan: f64_of(m.get("north_money").unwrap_or(&Value::Null)),
            south_money_wan: f64_of(m.get("south_money").unwrap_or(&Value::Null)),
            hgt_wan: f64_of(m.get("hgt").unwrap_or(&Value::Null)),
            sgt_wan: f64_of(m.get("sgt").unwrap_or(&Value::Null)),
        }))
    }

    /// `moneyflow_ind_dc` — industry-level main-force flow,
    /// ranked by net amount.
    pub async fn moneyflow_ind_dc(
        &self,
        trade_date: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IndustryFlowRow>, VendorError> {
        let mut params = serde_json::json!({});
        if let Some(d) = trade_date {
            params["trade_date"] = d.into();
        }
        let resp = self
            .post(
                "moneyflow_ind_dc",
                params,
                "trade_date,content_type,ts_code,name,pct_change,net_amount,net_amount_rate,rank,buy_sm_amount_stock",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "moneyflow_ind_dc: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                let ct = str_of(m.get("content_type").unwrap_or(&Value::Null))
                    .unwrap_or_default();
                if !ct.contains("行业") && !ct.is_empty() {
                    return None;
                }
                Some(IndustryFlowRow {
                    ts_code: str_of(m.get("ts_code").unwrap_or(&Value::Null))
                        .unwrap_or_default(),
                    name: str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
                    pct_change: f64_of(m.get("pct_change").unwrap_or(&Value::Null)),
                    net_amount_yuan: f64_of(m.get("net_amount").unwrap_or(&Value::Null)),
                    net_amount_rate: f64_of(m.get("net_amount_rate").unwrap_or(&Value::Null)),
                    rank: f64_of(m.get("rank").unwrap_or(&Value::Null)).map(|v| v as u32),
                    lead_stock: str_of(m.get("buy_sm_amount_stock").unwrap_or(&Value::Null)),
                })
            })
            .take(limit)
            .collect())
    }

    /// `limit_list_d` — today's up/down-limit list.
    pub async fn limit_list_d(
        &self,
        trade_date: &str,
    ) -> Result<Vec<LimitListRow>, VendorError> {
        let resp = self
            .post(
                "limit_list_d",
                serde_json::json!({ "trade_date": trade_date }),
                "trade_date,ts_code,name,close,pct_chg,turnover_ratio,limit",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "limit_list_d: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                LimitListRow {
                    trade_date: iso_date(
                        &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                    ),
                    ts_code: str_of(m.get("ts_code").unwrap_or(&Value::Null))
                        .unwrap_or_default(),
                    name: str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
                    close: f64_of(m.get("close").unwrap_or(&Value::Null)),
                    pct_chg: f64_of(m.get("pct_chg").unwrap_or(&Value::Null)),
                    turnover_ratio: f64_of(m.get("turnover_ratio").unwrap_or(&Value::Null)),
                    limit: str_of(m.get("limit").unwrap_or(&Value::Null)),
                }
            })
            .collect())
    }

    /// `limit_cpt_list` — strongest concept boards by consecutive limits.
    pub async fn limit_cpt_list(&self, trade_date: &str) -> Result<Vec<LimitCapt>, VendorError> {
        let resp = self
            .post(
                "limit_cpt_list",
                serde_json::json!({ "trade_date": trade_date }),
                "ts_code,name,trade_date,days,up_stat,cons_nums,up_nums,pct_change,rank",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "limit_cpt_list: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                LimitCapt {
                    ts_code: str_of(m.get("ts_code").unwrap_or(&Value::Null))
                        .unwrap_or_default(),
                    name: str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
                    days: f64_of(m.get("days").unwrap_or(&Value::Null)).map(|v| v as u32),
                    up_stat: str_of(m.get("up_stat").unwrap_or(&Value::Null)),
                    cons_nums: str_of(m.get("cons_nums").unwrap_or(&Value::Null)),
                    up_nums: f64_of(m.get("up_nums").unwrap_or(&Value::Null)).map(|v| v as u32),
                    pct_change: f64_of(m.get("pct_change").unwrap_or(&Value::Null)),
                }
            })
            .collect())
    }
}

// ── C. Industry landscape (industry_landscape tool) ──────────────────

impl TushareClient {
    /// `stock_basic` — full listed catalog (industry, name, exchange).
    /// We filter client-side because Tushare's `industry` param accepts
    /// only an exact match and there are common variants.
    pub async fn stock_basic_for_industry(
        &self,
        industry: &str,
    ) -> Result<Vec<(String, String)>, VendorError> {
        let resp = self
            .post(
                "stock_basic",
                serde_json::json!({ "industry": industry, "list_status": "L" }),
                "ts_code,name,industry",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stock_basic: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                let ts = str_of(m.get("ts_code").unwrap_or(&Value::Null))?;
                let name = str_of(m.get("name").unwrap_or(&Value::Null))?;
                Some((ts, name))
            })
            .collect())
    }

    /// `stock_basic` for one symbol — used to discover the symbol's
    /// industry.
    pub async fn stock_basic_one(
        &self,
        symbol: &Symbol,
    ) -> Result<(String, Option<String>, Option<String>), VendorError> {
        let resp = self
            .post(
                "stock_basic",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,name,industry,area",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stock_basic: empty data envelope")
        })?;
        let row = data
            .items
            .first()
            .ok_or_else(|| VendorError::recoverable(VENDOR, "stock_basic: no row for symbol"))?;
        let m = data.row_map(row);
        Ok((
            str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
            str_of(m.get("industry").unwrap_or(&Value::Null)),
            str_of(m.get("area").unwrap_or(&Value::Null)),
        ))
    }

    /// `daily_basic` for one symbol — latest valuation row.
    pub async fn daily_basic_one(
        &self,
        symbol: &Symbol,
    ) -> Result<BTreeMap<String, Option<f64>>, VendorError> {
        let resp = self
            .post(
                "daily_basic",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,trade_date,pe_ttm,pb,ps_ttm,dv_ttm,total_mv,circ_mv,turnover_rate",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "daily_basic: empty data envelope")
        })?;
        let Some(row) = data.items.first() else {
            return Err(VendorError::recoverable(VENDOR, "daily_basic: no row"));
        };
        let m = data.row_map(row);
        let mut out = BTreeMap::new();
        for k in ["pe_ttm", "pb", "ps_ttm", "dv_ttm", "turnover_rate"] {
            out.insert(k.into(), f64_of(m.get(k).unwrap_or(&Value::Null)));
        }
        // Convert 万元 → 元.
        for (vendor_k, neutral_k) in [("total_mv", "market_cap"), ("circ_mv", "float_market_cap")]
        {
            out.insert(
                neutral_k.into(),
                f64_of(m.get(vendor_k).unwrap_or(&Value::Null)).map(|v| v * 10_000.0),
            );
        }
        out.insert(
            "trade_date".into(),
            str_of(m.get("trade_date").unwrap_or(&Value::Null))
                .and_then(|s| s.parse::<f64>().ok()),
        );
        Ok(out)
    }

    /// Bulk `daily_basic` for a peer set — returns one row per ts_code.
    /// We loop one-by-one because tushare's multi-code call bundles
    /// historical rows together.
    pub async fn daily_basic_bulk(
        &self,
        ts_codes: &[String],
    ) -> Vec<(String, BTreeMap<String, Option<f64>>)> {
        let mut out = Vec::new();
        for ts in ts_codes {
            if let Ok(resp) = self
                .post(
                    "daily_basic",
                    serde_json::json!({ "ts_code": ts }),
                    "ts_code,trade_date,pe_ttm,pb,ps_ttm,dv_ttm,total_mv,turnover_rate",
                )
                .await
            {
                if let Some(data) = resp.data {
                    if let Some(row) = data.items.first() {
                        let m = data.row_map(row);
                        let mut row_map = BTreeMap::new();
                        for k in ["pe_ttm", "pb", "ps_ttm", "dv_ttm", "turnover_rate"] {
                            row_map.insert(k.into(), f64_of(m.get(k).unwrap_or(&Value::Null)));
                        }
                        row_map.insert(
                            "market_cap".into(),
                            f64_of(m.get("total_mv").unwrap_or(&Value::Null))
                                .map(|v| v * 10_000.0),
                        );
                        out.push((ts.clone(), row_map));
                    }
                }
            }
        }
        out
    }

    /// `fina_indicator` for one symbol — latest annual ratios. Kept
    /// available for `industry_landscape` "rank by ROE" extension; not
    /// wired in M4.1.1 (we rank by market cap there).
    #[allow(dead_code)]
    pub async fn fina_indicator_one(
        &self,
        ts_code: &str,
    ) -> Option<BTreeMap<String, Option<f64>>> {
        let resp = self
            .post(
                "fina_indicator",
                serde_json::json!({ "ts_code": ts_code }),
                "ts_code,end_date,roe,grossprofit_margin,netprofit_margin",
            )
            .await
            .ok()?;
        let data = resp.data?;
        let row = data.items.first()?;
        let m = data.row_map(row);
        let mut out = BTreeMap::new();
        out.insert("roe".into(), f64_of(m.get("roe").unwrap_or(&Value::Null)));
        out.insert(
            "gross_margin".into(),
            f64_of(m.get("grossprofit_margin").unwrap_or(&Value::Null)),
        );
        out.insert(
            "net_margin".into(),
            f64_of(m.get("netprofit_margin").unwrap_or(&Value::Null)),
        );
        Some(out)
    }

    /// `sw_daily` — SW industry index daily values for the given symbol
    /// list.
    pub async fn sw_daily(&self, trade_date: &str) -> Result<Vec<Value>, VendorError> {
        let resp = self
            .post(
                "sw_daily",
                serde_json::json!({ "trade_date": trade_date }),
                "ts_code,trade_date,name,close,pct_change,pe,pb",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "sw_daily: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                serde_json::json!({
                    "ts_code": str_of(m.get("ts_code").unwrap_or(&Value::Null)),
                    "name": str_of(m.get("name").unwrap_or(&Value::Null)),
                    "close": f64_of(m.get("close").unwrap_or(&Value::Null)),
                    "pct_change": f64_of(m.get("pct_change").unwrap_or(&Value::Null)),
                    "pe": f64_of(m.get("pe").unwrap_or(&Value::Null)),
                    "pb": f64_of(m.get("pb").unwrap_or(&Value::Null)),
                })
            })
            .collect())
    }

    /// `dc_concept` — EastMoney concept-board universe (~5000+ boards).
    /// Returns the full snapshot of the last trade day (newest first).
    /// Callers filter / rank client-side because the upstream only
    /// exposes name-equality and is missing a free-text search.
    pub async fn dc_concept(&self, limit: usize) -> Result<Vec<ConceptItem>, VendorError> {
        let resp = self
            .post(
                "dc_concept",
                serde_json::json!({ "src": "dc" }),
                "theme_code,trade_date,name,pct_change,hot,lead_stock,lead_stock_code,\
                 lead_stock_pct_change,main_change,z_t_num",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "dc_concept: empty data envelope")
        })?;
        Ok(rows_take(&data, limit)
            .map(|m| ConceptItem {
                theme_code: str_of(m.get("theme_code").unwrap_or(&Value::Null))
                    .unwrap_or_default(),
                trade_date: iso_date(
                    &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                name: str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
                pct_change: f64_of(m.get("pct_change").unwrap_or(&Value::Null)),
                hot: f64_of(m.get("hot").unwrap_or(&Value::Null)),
                lead_stock: str_of(m.get("lead_stock").unwrap_or(&Value::Null)),
                lead_stock_code: str_of(m.get("lead_stock_code").unwrap_or(&Value::Null)),
                lead_stock_pct: f64_of(m.get("lead_stock_pct_change").unwrap_or(&Value::Null)),
                main_change: f64_of(m.get("main_change").unwrap_or(&Value::Null)),
                z_t_num: f64_of(m.get("z_t_num").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `dc_concept_cons` — constituent stocks for one concept board.
    ///
    /// Audit caveat: this endpoint accepts ONLY `name` (the exact concept
    /// name string); passing `ts_code` returns 0 rows. We surface the
    /// raw count plus a small projection — callers truncate further if
    /// they don't need the entire member list.
    ///
    /// Held as `dead_code` for now: the M4.1.2 `industry_landscape`
    /// concepts focus uses the `lead_stock` already carried on the
    /// `dc_concept` row, so we avoid the extra per-pick HTTP round.
    /// Kept on the surface because a future "drill into a single concept
    /// board" tool would want it.
    #[allow(dead_code)]
    pub async fn dc_concept_cons(
        &self,
        name: &str,
        limit: usize,
    ) -> Result<Vec<ConceptConstituent>, VendorError> {
        let resp = self
            .post(
                "dc_concept_cons",
                serde_json::json!({ "name": name }),
                "ts_code,name,industry,reason",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "dc_concept_cons: empty data envelope")
        })?;
        Ok(rows_take(&data, limit)
            .map(|m| ConceptConstituent {
                ts_code: str_of(m.get("ts_code").unwrap_or(&Value::Null)).unwrap_or_default(),
                name: str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
                industry: str_of(m.get("industry").unwrap_or(&Value::Null)),
                reason: str_of(m.get("reason").unwrap_or(&Value::Null)),
            })
            .collect())
    }
}

// ── D. Individual-stock dossier helpers (`stock_overview`) ───────────

impl TushareClient {
    /// `stock_company` — extended company profile (chairman / secretary
    /// / introduction / business scope).
    pub async fn stock_company(
        &self,
        symbol: &Symbol,
    ) -> Result<Option<CompanyProfile>, VendorError> {
        let resp = self
            .post(
                "stock_company",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,com_name,chairman,secretary,reg_capital,setup_date,province,city,introduction,website,employees,main_business,exchange",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stock_company: empty data envelope")
        })?;
        let Some(row) = data.items.first() else { return Ok(None) };
        let m = data.row_map(row);
        Ok(Some(CompanyProfile {
            symbol: symbol.to_dotted(),
            name: str_of(m.get("com_name").unwrap_or(&Value::Null)).unwrap_or_default(),
            chairman: str_of(m.get("chairman").unwrap_or(&Value::Null)),
            secretary: str_of(m.get("secretary").unwrap_or(&Value::Null)),
            industry: None,
            area: None,
            exchange_label: str_of(m.get("exchange").unwrap_or(&Value::Null)).map(|raw| {
                match raw.as_str() {
                    "SSE" => "上海证券交易所".into(),
                    "SZSE" => "深圳证券交易所".into(),
                    "BSE" => "北京证券交易所".into(),
                    _ => raw,
                }
            }),
            list_date: None,
            introduction: str_of(m.get("introduction").unwrap_or(&Value::Null)),
            main_business: str_of(m.get("main_business").unwrap_or(&Value::Null)),
            website: str_of(m.get("website").unwrap_or(&Value::Null)),
            employees: f64_of(m.get("employees").unwrap_or(&Value::Null)),
            fullname: None,
        }))
    }

    /// Three statements (income / balance / cashflow / ratios). Returns
    /// rows newest-first, count-capped.
    pub async fn fina_statement(
        &self,
        symbol: &Symbol,
        statement: &str,
        period: &str,
        count: usize,
    ) -> Result<Vec<FinancialRow>, VendorError> {
        let (api, fields, key_map): (&str, &str, &[(&str, &str)]) = match statement {
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
                "ts_code,end_date,roe,roa,grossprofit_margin,netprofit_margin,debt_to_assets,current_ratio,quick_ratio,or_yoy,netprofit_yoy",
                &[
                    ("roe", "roe"),
                    ("roa", "roa"),
                    ("gross_margin", "grossprofit_margin"),
                    ("net_margin", "netprofit_margin"),
                    ("debt_to_assets", "debt_to_assets"),
                    ("current_ratio", "current_ratio"),
                    ("quick_ratio", "quick_ratio"),
                    ("revenue_yoy", "or_yoy"),
                    ("net_profit_yoy", "netprofit_yoy"),
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
            serde_json::json!({ "ts_code": symbol.to_dotted(), "period": "1231" })
        } else {
            serde_json::json!({ "ts_code": symbol.to_dotted() })
        };
        let resp = self.post(api, params, fields).await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, format!("{api}: empty data envelope"))
        })?;
        let filter_annual = period == "year";
        Ok(data
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
            .collect())
    }

    /// `fina_mainbz` — main business breakdown for one symbol.
    pub async fn fina_mainbz(
        &self,
        symbol: &Symbol,
        dim: &str,
    ) -> Result<Vec<super::types::BusinessRow>, VendorError> {
        let bz_item = match dim {
            "product" => "P",
            "industry" => "I",
            "region" => "R",
            other => {
                return Err(VendorError::fatal(
                    VENDOR,
                    format!(
                        "unsupported breakdown dim '{other}' (try product/industry/region)"
                    ),
                ))
            }
        };
        let resp = self
            .post(
                "fina_mainbz",
                serde_json::json!({ "ts_code": symbol.to_dotted(), "type": bz_item }),
                "ts_code,end_date,bz_item,bz_sales,bz_profit,bz_cost",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "fina_mainbz: empty data envelope")
        })?;
        if data.items.is_empty() {
            return Err(VendorError::recoverable(
                VENDOR,
                "fina_mainbz: no rows for symbol",
            ));
        }
        let latest = data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                str_of(m.get("end_date").unwrap_or(&Value::Null))
            })
            .max()
            .unwrap_or_default();
        let scoped: Vec<_> = data
            .items
            .iter()
            .filter(|row| {
                let m = data.row_map(row);
                str_of(m.get("end_date").unwrap_or(&Value::Null))
                    .map(|e| e == latest)
                    .unwrap_or(false)
            })
            .collect();
        let total: f64 = scoped
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                f64_of(m.get("bz_sales").unwrap_or(&Value::Null))
            })
            .sum();
        Ok(scoped
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                let sales = f64_of(m.get("bz_sales").unwrap_or(&Value::Null));
                let cost = f64_of(m.get("bz_cost").unwrap_or(&Value::Null));
                let gm = match (sales, cost) {
                    (Some(s), Some(c)) if s > 0.0 => Some((s - c) / s * 100.0),
                    _ => None,
                };
                let pct = sales.and_then(|s| {
                    if total > 0.0 {
                        Some(s / total * 100.0)
                    } else {
                        None
                    }
                });
                super::types::BusinessRow {
                    dimension: dim.to_string(),
                    item: str_of(m.get("bz_item").unwrap_or(&Value::Null))
                        .unwrap_or_else(|| "(未知)".into()),
                    revenue_yuan: sales,
                    pct_of_total: pct,
                    gross_margin_pct: gm,
                    period_end: iso_date(&latest),
                }
            })
            .collect())
    }

    /// `top10_holders` — newest period top-10 (vendor returns multiple
    /// historical periods; we keep latest).
    pub async fn top10_holders(
        &self,
        symbol: &Symbol,
        kind: &str,
    ) -> Result<(String, Vec<HolderRow>), VendorError> {
        let api = match kind {
            "total" => "top10_holders",
            "float" => "top10_floatholders",
            other => {
                return Err(VendorError::fatal(
                    VENDOR,
                    format!("unsupported holders kind '{other}' (try total/float)"),
                ))
            }
        };
        let resp = self
            .post(
                api,
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,end_date,holder_name,holder_type,hold_amount,hold_ratio,hold_change",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, format!("{api}: empty data envelope"))
        })?;
        if data.items.is_empty() {
            return Err(VendorError::recoverable(
                VENDOR,
                format!("{api}: no holder rows"),
            ));
        }
        let latest = data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                str_of(m.get("end_date").unwrap_or(&Value::Null))
            })
            .max()
            .unwrap_or_default();
        let holders: Vec<HolderRow> = data
            .items
            .iter()
            .filter(|row| {
                let m = data.row_map(row);
                str_of(m.get("end_date").unwrap_or(&Value::Null))
                    .map(|e| e == latest)
                    .unwrap_or(false)
            })
            .enumerate()
            .map(|(idx, row)| {
                let m = data.row_map(row);
                HolderRow {
                    rank: (idx + 1) as u32,
                    holder_name: str_of(m.get("holder_name").unwrap_or(&Value::Null))
                        .unwrap_or_else(|| "(未知)".into()),
                    holder_type: str_of(m.get("holder_type").unwrap_or(&Value::Null)),
                    shares: f64_of(m.get("hold_amount").unwrap_or(&Value::Null)),
                    pct: f64_of(m.get("hold_ratio").unwrap_or(&Value::Null)),
                    change_qoq_shares: f64_of(m.get("hold_change").unwrap_or(&Value::Null)),
                }
            })
            .take(10)
            .collect();
        Ok((iso_date(&latest), holders))
    }

    /// `stk_holdernumber` — total shareholder count history (newest first).
    pub async fn stk_holdernumber(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<HolderCountObs>, VendorError> {
        let resp = self
            .post(
                "stk_holdernumber",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,end_date,holder_num",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stk_holdernumber: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| HolderCountObs {
                end_date: iso_date(
                    &str_of(m.get("end_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                holder_count: f64_of(m.get("holder_num").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `daily` — OHLCV bars (count-capped). Newest first per vendor.
    pub async fn daily(
        &self,
        symbol: &Symbol,
        count: usize,
    ) -> Result<Vec<Candle>, VendorError> {
        let resp = self
            .post(
                "daily",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,trade_date,open,high,low,close,vol,amount",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "daily: empty data envelope")
        })?;
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
                    turnover: f64_of(m.get("amount").unwrap_or(&Value::Null)).map(|v| v * 1000.0),
                })
            })
            .collect();
        candles.reverse();
        if candles.is_empty() {
            return Err(VendorError::recoverable(VENDOR, "daily: no candles"));
        }
        Ok(candles)
    }

    /// `weekly` / `monthly` OHLCV (count-capped).
    pub async fn period_candles(
        &self,
        symbol: &Symbol,
        period: &str,
        count: usize,
    ) -> Result<Vec<Candle>, VendorError> {
        let api = match period {
            "weekly" => "weekly",
            "monthly" => "monthly",
            other => {
                return Err(VendorError::fatal(
                    VENDOR,
                    format!("unsupported candle period '{other}' (try weekly/monthly)"),
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
                    turnover: f64_of(m.get("amount").unwrap_or(&Value::Null)).map(|v| v * 1000.0),
                })
            })
            .collect();
        candles.reverse();
        if candles.is_empty() {
            return Err(VendorError::recoverable(VENDOR, format!("{api}: no candles")));
        }
        Ok(candles)
    }

    /// `stk_factor` — technical indicators row stream.
    pub async fn stk_factor(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<TechRow>, VendorError> {
        let resp = self
            .post(
                "stk_factor",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,trade_date,close,macd_dif,macd_dea,macd,kdj_k,kdj_d,kdj_j,rsi_6,rsi_12,rsi_24,boll_upper,boll_mid,boll_lower",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stk_factor: empty data envelope")
        })?;
        let mut out: Vec<TechRow> = rows_take(&data, rows)
            .map(|m| TechRow {
                date: iso_date(
                    &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                close: f64_of(m.get("close").unwrap_or(&Value::Null)),
                macd_dif: f64_of(m.get("macd_dif").unwrap_or(&Value::Null)),
                macd_dea: f64_of(m.get("macd_dea").unwrap_or(&Value::Null)),
                macd: f64_of(m.get("macd").unwrap_or(&Value::Null)),
                kdj_k: f64_of(m.get("kdj_k").unwrap_or(&Value::Null)),
                kdj_d: f64_of(m.get("kdj_d").unwrap_or(&Value::Null)),
                kdj_j: f64_of(m.get("kdj_j").unwrap_or(&Value::Null)),
                rsi_6: f64_of(m.get("rsi_6").unwrap_or(&Value::Null)),
                rsi_12: f64_of(m.get("rsi_12").unwrap_or(&Value::Null)),
                rsi_24: f64_of(m.get("rsi_24").unwrap_or(&Value::Null)),
                boll_upper: f64_of(m.get("boll_upper").unwrap_or(&Value::Null)),
                boll_mid: f64_of(m.get("boll_mid").unwrap_or(&Value::Null)),
                boll_lower: f64_of(m.get("boll_lower").unwrap_or(&Value::Null)),
            })
            .collect();
        out.reverse();
        Ok(out)
    }
}

// ── E. Recent actions (`recent_actions`) ─────────────────────────────

impl TushareClient {
    /// `anns_d` — A-share announcement bulletin board for one symbol.
    pub async fn anns_d(
        &self,
        symbol: &Symbol,
        start: &str,
        end: &str,
    ) -> Result<Vec<AnnouncementItem>, VendorError> {
        let resp = self
            .post(
                "anns_d",
                serde_json::json!({
                    "ts_code": symbol.to_dotted(),
                    "start_date": start,
                    "end_date": end,
                }),
                "ts_code,ann_date,title,url",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "anns_d: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                let title = str_of(m.get("title").unwrap_or(&Value::Null))?;
                let date = iso_date(
                    &str_of(m.get("ann_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                );
                let url = str_of(m.get("url").unwrap_or(&Value::Null));
                Some(AnnouncementItem {
                    date,
                    category: Some(classify_announcement(&title)),
                    title,
                    url,
                    pdf_url: None,
                })
            })
            .collect())
    }

    /// `stk_holdertrade` — 股东增减持.
    pub async fn stk_holdertrade(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<InsiderTradeItem>, VendorError> {
        let resp = self
            .post(
                "stk_holdertrade",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,holder_name,in_de,change_vol,change_ratio,after_share,after_ratio",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stk_holdertrade: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| InsiderTradeItem {
                ann_date: iso_date(
                    &str_of(m.get("ann_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                holder_name: str_of(m.get("holder_name").unwrap_or(&Value::Null))
                    .unwrap_or_default(),
                direction: str_of(m.get("in_de").unwrap_or(&Value::Null)),
                change_vol_shares: f64_of(m.get("change_vol").unwrap_or(&Value::Null)),
                change_ratio_pct: f64_of(m.get("change_ratio").unwrap_or(&Value::Null)),
                after_shares: f64_of(m.get("after_share").unwrap_or(&Value::Null)),
                after_ratio_pct: f64_of(m.get("after_ratio").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `dividend` — 分红送股 events.
    pub async fn dividend(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<DividendItem>, VendorError> {
        let resp = self
            .post(
                "dividend",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,end_date,ann_date,div_proc,stk_div,cash_div,ex_date,pay_date",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "dividend: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| DividendItem {
                end_date: iso_date(
                    &str_of(m.get("end_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                ann_date: iso_date(
                    &str_of(m.get("ann_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                process: str_of(m.get("div_proc").unwrap_or(&Value::Null)),
                stk_div: f64_of(m.get("stk_div").unwrap_or(&Value::Null)),
                cash_div_pretax: f64_of(m.get("cash_div").unwrap_or(&Value::Null)),
                ex_date: str_of(m.get("ex_date").unwrap_or(&Value::Null)).map(|s| iso_date(&s)),
                pay_date: str_of(m.get("pay_date").unwrap_or(&Value::Null)).map(|s| iso_date(&s)),
            })
            .collect())
    }

    /// `share_float` — 限售解禁 events.
    pub async fn share_float(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<ShareUnlockItem>, VendorError> {
        let resp = self
            .post(
                "share_float",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,float_date,float_share,float_ratio,holder_name,share_type",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "share_float: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| ShareUnlockItem {
                ann_date: iso_date(
                    &str_of(m.get("ann_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                float_date: iso_date(
                    &str_of(m.get("float_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                float_share: f64_of(m.get("float_share").unwrap_or(&Value::Null)),
                float_ratio: f64_of(m.get("float_ratio").unwrap_or(&Value::Null)),
                holder_name: str_of(m.get("holder_name").unwrap_or(&Value::Null)),
                share_type: str_of(m.get("share_type").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `block_trade` — 大宗交易 records.
    pub async fn block_trade(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<BlockTradeItem>, VendorError> {
        let resp = self
            .post(
                "block_trade",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,trade_date,price,vol,amount,buyer,seller",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "block_trade: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| BlockTradeItem {
                trade_date: iso_date(
                    &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                price: f64_of(m.get("price").unwrap_or(&Value::Null)),
                vol_wan_shares: f64_of(m.get("vol").unwrap_or(&Value::Null)),
                amount_wan_yuan: f64_of(m.get("amount").unwrap_or(&Value::Null)),
                buyer: str_of(m.get("buyer").unwrap_or(&Value::Null)),
                seller: str_of(m.get("seller").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `top_list` — 龙虎榜 entries. Filter mode = per-symbol.
    pub async fn top_list_symbol(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<TopListItem>, VendorError> {
        let resp = self
            .post(
                "top_list",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,trade_date,close,pct_change,turnover_rate,net_amount,l_buy,l_sell,reason",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "top_list: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| TopListItem {
                trade_date: iso_date(
                    &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                close: f64_of(m.get("close").unwrap_or(&Value::Null)),
                pct_change: f64_of(m.get("pct_change").unwrap_or(&Value::Null)),
                turnover_rate: f64_of(m.get("turnover_rate").unwrap_or(&Value::Null)),
                net_amount: f64_of(m.get("net_amount").unwrap_or(&Value::Null)),
                l_buy: f64_of(m.get("l_buy").unwrap_or(&Value::Null)),
                l_sell: f64_of(m.get("l_sell").unwrap_or(&Value::Null)),
                reason: str_of(m.get("reason").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `pledge_stat` — 股权质押 status.
    pub async fn pledge_stat(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<PledgeStatItem>, VendorError> {
        let resp = self
            .post(
                "pledge_stat",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,end_date,pledge_count,unrest_pledge,rest_pledge,total_share,pledge_ratio",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "pledge_stat: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| PledgeStatItem {
                end_date: iso_date(
                    &str_of(m.get("end_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                pledge_count: f64_of(m.get("pledge_count").unwrap_or(&Value::Null)),
                unrest_pledge_wan: f64_of(m.get("unrest_pledge").unwrap_or(&Value::Null)),
                rest_pledge_wan: f64_of(m.get("rest_pledge").unwrap_or(&Value::Null)),
                total_share_wan: f64_of(m.get("total_share").unwrap_or(&Value::Null)),
                pledge_ratio_pct: f64_of(m.get("pledge_ratio").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `repurchase` — 公司回购 records.
    pub async fn repurchase(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<RepurchaseItem>, VendorError> {
        let resp = self
            .post(
                "repurchase",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,end_date,proc,vol,amount,high_limit,low_limit",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "repurchase: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| RepurchaseItem {
                ann_date: iso_date(
                    &str_of(m.get("ann_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                end_date: str_of(m.get("end_date").unwrap_or(&Value::Null)).map(|s| iso_date(&s)),
                proc: str_of(m.get("proc").unwrap_or(&Value::Null)),
                vol_share: f64_of(m.get("vol").unwrap_or(&Value::Null)),
                amount_yuan: f64_of(m.get("amount").unwrap_or(&Value::Null)),
                high_limit: f64_of(m.get("high_limit").unwrap_or(&Value::Null)),
                low_limit: f64_of(m.get("low_limit").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `stk_surv` — institution visit log. Per the audit, the only
    /// dependable axis is `trade_date` / range; ts_code single-shot
    /// returns 0 rows. We pull by date window then filter to symbol.
    pub async fn stk_surv_window(
        &self,
        symbol_filter: Option<&Symbol>,
        start_date: &str,
        end_date: &str,
    ) -> Result<Vec<InstitutionVisitItem>, VendorError> {
        let resp = self
            .post(
                "stk_surv",
                serde_json::json!({ "start_date": start_date, "end_date": end_date }),
                "ts_code,name,surv_date,fund_visitors,rece_place,rece_mode,rece_org,org_type,comp_rece",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "stk_surv: empty data envelope")
        })?;
        let want = symbol_filter.map(|s| s.to_dotted());
        Ok(data
            .items
            .iter()
            .filter_map(|row| {
                let m = data.row_map(row);
                let ts = str_of(m.get("ts_code").unwrap_or(&Value::Null))?;
                if let Some(w) = &want {
                    if &ts != w {
                        return None;
                    }
                }
                Some(InstitutionVisitItem {
                    surv_date: iso_date(
                        &str_of(m.get("surv_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                    ),
                    receivers: str_of(m.get("fund_visitors").unwrap_or(&Value::Null)),
                    place: str_of(m.get("rece_place").unwrap_or(&Value::Null)),
                    mode: str_of(m.get("rece_mode").unwrap_or(&Value::Null)),
                    visiting_org: str_of(m.get("rece_org").unwrap_or(&Value::Null)),
                    org_type: str_of(m.get("org_type").unwrap_or(&Value::Null)),
                    host: str_of(m.get("comp_rece").unwrap_or(&Value::Null)),
                })
            })
            .collect())
    }
}

// ── F. Research / sentiment (`research_sentiment`) ───────────────────

impl TushareClient {
    /// `report_rc` — sell-side consensus. **Rate-limited 1/min** — tool
    /// layer must hold a token bucket; this method does not.
    pub async fn report_rc(
        &self,
        symbol: &Symbol,
    ) -> Result<(Vec<YearForecast>, RatingMix, usize), VendorError> {
        let resp = self
            .post(
                "report_rc",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,name,report_date,report_title,quarter,eps,pe,np,rating,max_price",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "report_rc: empty data envelope")
        })?;
        let mut eps_by_year: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut np_by_year: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut size_by_year: BTreeMap<String, usize> = BTreeMap::new();
        let mut rating_mix = RatingMix::default();
        for row in &data.items {
            let m = data.row_map(row);
            let quarter = str_of(m.get("quarter").unwrap_or(&Value::Null)).unwrap_or_default();
            let year = if quarter.len() >= 4 {
                quarter[..4].to_string()
            } else {
                continue;
            };
            *size_by_year.entry(year.clone()).or_default() += 1;
            if let Some(eps) = f64_of(m.get("eps").unwrap_or(&Value::Null)) {
                eps_by_year.entry(year.clone()).or_default().push(eps);
            }
            if let Some(np) = f64_of(m.get("np").unwrap_or(&Value::Null)) {
                np_by_year.entry(year).or_default().push(np);
            }
            let rating = str_of(m.get("rating").unwrap_or(&Value::Null)).unwrap_or_default();
            bump_rating(&mut rating_mix, &rating);
        }
        let mut years: std::collections::BTreeSet<String> =
            eps_by_year.keys().chain(np_by_year.keys()).cloned().collect();
        let _ = years.insert("".to_string());
        let mut out: Vec<YearForecast> = Vec::new();
        for year in size_by_year.keys() {
            let eps = eps_by_year.get(year).cloned().unwrap_or_default();
            let np = np_by_year.get(year).cloned().unwrap_or_default();
            let sample = *size_by_year.get(year).unwrap_or(&0);
            out.push(YearForecast {
                year: year.clone(),
                eps_mean: mean(&eps),
                eps_high: eps.iter().cloned().reduce(f64::max),
                eps_low: eps.iter().cloned().reduce(f64::min),
                net_profit_mean_yi_yuan: mean(&np),
                net_profit_high_yi_yuan: np.iter().cloned().reduce(f64::max),
                net_profit_low_yi_yuan: np.iter().cloned().reduce(f64::min),
                sample_size: sample,
            });
        }
        Ok((out, rating_mix, data.items.len()))
    }

    /// `research_report` — listing of broker research reports.
    pub async fn research_report(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<ResearchReportItem>, VendorError> {
        let resp = self
            .post(
                "research_report",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "trade_date,title,report_type,author,name,ts_code,inst_csname,url",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "research_report: empty data envelope")
        })?;
        Ok(rows_take(&data, rows)
            .map(|m| ResearchReportItem {
                date: iso_date(
                    &str_of(m.get("trade_date").unwrap_or(&Value::Null)).unwrap_or_default(),
                ),
                title: str_of(m.get("title").unwrap_or(&Value::Null)).unwrap_or_default(),
                org_name: str_of(m.get("inst_csname").unwrap_or(&Value::Null)),
                author: str_of(m.get("author").unwrap_or(&Value::Null)),
                rating: None,
                target_price: None,
                pdf_url: str_of(m.get("url").unwrap_or(&Value::Null)),
            })
            .collect())
    }

    /// `broker_recommend` — monthly broker top picks.
    pub async fn broker_recommend(
        &self,
        month: &str,
    ) -> Result<Vec<BrokerMonthlyPick>, VendorError> {
        let resp = self
            .post(
                "broker_recommend",
                serde_json::json!({ "month": month }),
                "month,broker,ts_code,name",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "broker_recommend: empty data envelope")
        })?;
        Ok(data
            .items
            .iter()
            .map(|row| {
                let m = data.row_map(row);
                BrokerMonthlyPick {
                    month: str_of(m.get("month").unwrap_or(&Value::Null)).unwrap_or_default(),
                    broker: str_of(m.get("broker").unwrap_or(&Value::Null)).unwrap_or_default(),
                    ts_code: str_of(m.get("ts_code").unwrap_or(&Value::Null)).unwrap_or_default(),
                    name: str_of(m.get("name").unwrap_or(&Value::Null)).unwrap_or_default(),
                }
            })
            .collect())
    }

    /// `forecast` — 业绩预告.
    pub async fn forecast(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<BTreeMap<String, Value>>, VendorError> {
        let resp = self
            .post(
                "forecast",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,end_date,type,p_change_min,p_change_max,net_profit_min,net_profit_max,summary",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "forecast: empty data envelope")
        })?;
        Ok(rows_take(&data, rows).collect())
    }

    /// `express` — 业绩快报.
    pub async fn express(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<BTreeMap<String, Value>>, VendorError> {
        let resp = self
            .post(
                "express",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,end_date,revenue,operate_profit,total_profit,n_income,yoy_net_profit",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "express: empty data envelope")
        })?;
        Ok(rows_take(&data, rows).collect())
    }

    /// `disclosure_date` — 财报披露日历.
    pub async fn disclosure_date(
        &self,
        symbol: &Symbol,
        rows: usize,
    ) -> Result<Vec<BTreeMap<String, Value>>, VendorError> {
        let resp = self
            .post(
                "disclosure_date",
                serde_json::json!({ "ts_code": symbol.to_dotted() }),
                "ts_code,ann_date,end_date,pre_date,actual_date,modify_date",
            )
            .await?;
        let data = resp.data.ok_or_else(|| {
            VendorError::recoverable(VENDOR, "disclosure_date: empty data envelope")
        })?;
        Ok(rows_take(&data, rows).collect())
    }
}

// ── Shared helpers ───────────────────────────────────────────────────

fn derive_label(end_date: &str, period: &str) -> String {
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

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

/// Best-effort categorization by title keywords.
pub(crate) fn classify_announcement(title: &str) -> String {
    let pairs: &[(&str, &str)] = &[
        ("分红", "分红配股"),
        ("配股", "分红配股"),
        ("增持", "增减持"),
        ("减持", "增减持"),
        ("回购", "回购"),
        ("解禁", "解禁"),
        ("董事", "高管/治理"),
        ("监事", "高管/治理"),
        ("立案", "监管"),
        ("处罚", "监管"),
        ("收购", "重大事项"),
        ("重组", "重大事项"),
        ("年报", "财报"),
        ("季报", "财报"),
        ("半年报", "财报"),
    ];
    for (k, label) in pairs {
        if title.contains(k) {
            return (*label).to_string();
        }
    }
    "其它".to_string()
}

pub(crate) fn bump_rating(mix: &mut RatingMix, rating: &str) {
    let r = rating.to_lowercase();
    if r.contains("buy") || rating.contains("买入") || rating.contains("强烈推荐") {
        mix.buy += 1;
    } else if r.contains("overweight") || rating.contains("增持") || rating.contains("推荐") {
        mix.overweight += 1;
    } else if r.contains("hold") || r.contains("neutral") || rating.contains("中性")
        || rating.contains("持有")
    {
        mix.hold += 1;
    } else if r.contains("underweight") || rating.contains("减持") {
        mix.underweight += 1;
    } else if r.contains("sell") || rating.contains("卖出") {
        mix.sell += 1;
    }
}

/// Public access for the eastmoney module (re-uses our taxonomy).
pub(crate) fn classify_ann_title(title: &str) -> String {
    classify_announcement(title)
}

// ── Unused helpers that fan-out tools want — `LiveQuote` is filled by
// the eastmoney module, not tushare. ─────────────────────────────────

impl TushareClient {
    /// `daily` snapshot → coerce into a LiveQuote (last EOD close).
    /// Used as the offline fallback for market_pulse / stock_overview
    /// when the eastmoney push2 endpoint is unreachable.
    pub async fn live_quote_eod(&self, symbol: &Symbol) -> Result<LiveQuote, VendorError> {
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
        let Some(row) = data.items.first() else {
            return Err(VendorError::recoverable(VENDOR, "daily: no rows"));
        };
        let m = data.row_map(row);
        Ok(LiveQuote {
            symbol: symbol.to_dotted(),
            name: None,
            price: f64_of(m.get("close").unwrap_or(&Value::Null)),
            change: f64_of(m.get("change").unwrap_or(&Value::Null)),
            change_pct: f64_of(m.get("pct_chg").unwrap_or(&Value::Null)),
            open: f64_of(m.get("open").unwrap_or(&Value::Null)),
            high: f64_of(m.get("high").unwrap_or(&Value::Null)),
            low: f64_of(m.get("low").unwrap_or(&Value::Null)),
            prev_close: f64_of(m.get("pre_close").unwrap_or(&Value::Null)),
            volume_shares: f64_of(m.get("vol").unwrap_or(&Value::Null)).map(|v| v * 100.0),
            turnover_yuan: f64_of(m.get("amount").unwrap_or(&Value::Null)).map(|v| v * 1000.0),
            turnover_rate: None,
            timestamp_unix: None,
        })
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
        assert_eq!(iso_date("2026-05-20"), "2026-05-20");
    }

    #[test]
    fn row_map_zips_fields_and_values() {
        let data = fake_response(
            vec!["a", "b", "c"],
            vec![vec![
                Value::Number(1.into()),
                Value::String("x".into()),
                Value::Null,
            ]],
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
    fn classify_announcement_routes_keywords() {
        assert_eq!(classify_announcement("XX 年度分红预案"), "分红配股");
        assert_eq!(classify_announcement("董事会会议决议"), "高管/治理");
        assert_eq!(classify_announcement("关于回购股份的进展"), "回购");
        assert_eq!(classify_announcement("XX 年报全文"), "财报");
        assert_eq!(classify_announcement("无关公告"), "其它");
    }

    #[test]
    fn bump_rating_normalizes_brokerage_taxonomy() {
        let mut mix = RatingMix::default();
        bump_rating(&mut mix, "Buy");
        bump_rating(&mut mix, "买入");
        bump_rating(&mut mix, "增持");
        bump_rating(&mut mix, "Hold");
        bump_rating(&mut mix, "Sell");
        assert_eq!(mix.buy, 2);
        assert_eq!(mix.overweight, 1);
        assert_eq!(mix.hold, 1);
        assert_eq!(mix.sell, 1);
    }
}
