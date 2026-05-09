use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_financials";
const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_PERIODS: u64 = 4;
const MAX_PERIODS: u64 = 24;

// SEC EDGAR — free, official, no API key. The companyfacts endpoint returns
// every us-gaap fact ever filed for a registrant. Requires a UA with a
// contact email; the parser is finicky so we mirror the format
// `sec_filing_fetch.rs` already uses.
const SEC_TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const SEC_COMPANYFACTS_URL: &str = "https://data.sec.gov/api/xbrl/companyfacts/CIK{cik}.json";
const SEC_USER_AGENT: &str = "Leek Research gradschool.hchen13@gmail.com";
const SEC_TICKER_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub struct GetFinancialsTool {
    http: Client,
    sec_http: Client,
    sec_ticker_cache: Mutex<Option<SecTickerCache>>,
}

struct SecTickerCache {
    map: HashMap<String, u64>,
    fetched_at: Instant,
}

impl GetFinancialsTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        let sec_ua = std::env::var("LEEK_SEC_USER_AGENT").unwrap_or_else(|_| SEC_USER_AGENT.into());
        let sec_http = Client::builder()
            .user_agent(sec_ua)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self {
            http,
            sec_http,
            sec_ticker_cache: Mutex::new(None),
        })
    }
}

#[async_trait]
impl ToolHandler for GetFinancialsTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch financial statements and key ratios for a listed company.\n\
                Covers A-shares (via Tushare Pro) and US stocks (via SEC EDGAR companyfacts).\n\
                - A-share ts_code: \"600519.SH\", \"000001.SZ\"\n\
                - US ticker: \"AAPL\", \"TSLA\", \"NVDA\"\n\
                report_type controls what to fetch:\n\
                - \"income\": income statement (revenue, profit, EPS)\n\
                - \"balance\": balance sheet (assets, liabilities, equity)\n\
                - \"cashflow\": cash flow statement\n\
                - \"ratios\": key financial ratios (margins, ROE, ROA, leverage)\n\
                - \"all\": fetch income + balance sheet + cash flow + ratios\n\
                Returns markdown tables with the most recent N periods (annual 10-K \
                preferred, falls back to quarterly 10-Q when annual data is sparse)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ticker": {"type": "string"},
                    "market": {
                        "type": "string",
                        "enum": ["a_share", "us_stock"]
                    },
                    "report_type": {
                        "type": "string",
                        "enum": ["income", "balance", "cashflow", "ratios", "all"],
                        "description": "Default: all"
                    },
                    "periods": {
                        "type": "integer",
                        "description": "Number of reporting periods (default 4, max 8)"
                    }
                },
                "required": ["ticker", "market"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let ticker = args
            .get("ticker")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'ticker' argument"))?
            .trim()
            .to_string();
        let market = args
            .get("market")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'market' argument"))?;
        let report_type = args
            .get("report_type")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let periods = args
            .get("periods")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, MAX_PERIODS))
            .unwrap_or(DEFAULT_PERIODS) as usize;

        match market {
            "a_share" => fetch_a_share(&self.http, &ticker, report_type, periods, cancel).await,
            "us_stock" => self.fetch_us_stock_via_sec(&ticker, report_type, periods, cancel).await,
            _ => bail!("unknown market: {market}"),
        }
    }
}

// ── A-share (Tushare) ──────────────────────────────────────────────────────

async fn fetch_a_share(
    http: &Client,
    ts_code: &str,
    report_type: &str,
    periods: usize,
    cancel: CancellationToken,
) -> Result<String> {
    let token = std::env::var("TUSHARE_TOKEN").map_err(|_| {
        anyhow!(
            "[get_financials: TUSHARE_TOKEN not set — A-share financials unavailable. \
             Get a free token at https://tushare.pro/register]"
        )
    })?;

    let mut parts: Vec<String> = Vec::new();

    match report_type {
        "income" => {
            parts.push(fetch_ashare_income(http, ts_code, periods, &token, &cancel).await?);
        }
        "balance" => {
            parts.push(fetch_ashare_balance(http, ts_code, periods, &token, &cancel).await?);
        }
        "cashflow" => {
            parts.push(fetch_ashare_cashflow(http, ts_code, periods, &token, &cancel).await?);
        }
        "ratios" => {
            parts.push(fetch_ashare_ratios(http, ts_code, periods, &token, &cancel).await?);
        }
        "all" => {
            parts.push(fetch_ashare_income(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_balance(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_cashflow(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_ratios(http, ts_code, periods, &token, &cancel).await?);
        }
        _ => bail!("unknown report_type: {report_type}"),
    }

    Ok(parts.join("\n\n"))
}

async fn tushare_call(
    http: &Client,
    token: &str,
    api_name: &str,
    ts_code: &str,
    periods: usize,
    fields: &str,
    cancel: &CancellationToken,
) -> Result<serde_json::Value> {
    let payload = serde_json::json!({
        "api_name": api_name,
        "token": token,
        "params": {"ts_code": ts_code, "limit": periods},
        "fields": fields,
    });

    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("aborted"),
        r = http.post(TUSHARE_ENDPOINT).json(&payload).send() => r?,
    };
    if !resp.status().is_success() {
        bail!("tushare HTTP {}", resp.status().as_u16());
    }
    let body: serde_json::Value = tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("aborted"),
        r = resp.json() => r?,
    };
    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        bail!("tushare error (code={code}): {msg}");
    }
    Ok(body)
}

fn tushare_extract(body: &serde_json::Value) -> (Vec<String>, Vec<Vec<serde_json::Value>>) {
    let data = match body.get("data") {
        Some(d) => d,
        None => return (vec![], vec![]),
    };
    let fields: Vec<String> = data
        .get("fields")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let items: Vec<Vec<serde_json::Value>> = data
        .get("items")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|row| row.as_array().cloned())
                .collect()
        })
        .unwrap_or_default();
    (fields, items)
}

fn tushare_fetch_limit(periods: usize) -> usize {
    (periods * 3).max(periods)
}

fn unique_rows_by_field(
    rows: Vec<Vec<serde_json::Value>>,
    fields: &[String],
    field: &str,
    limit: usize,
) -> Vec<Vec<serde_json::Value>> {
    let Some(index) = fields.iter().position(|f| f == field) else {
        return rows.into_iter().take(limit).collect();
    };
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|row| {
            let key = row.get(index).map(fmt_val_str).unwrap_or_default();
            !key.is_empty() && seen.insert(key)
        })
        .take(limit)
        .collect()
}

fn col<'a>(row: &'a [serde_json::Value], fields: &[String], name: &str) -> &'a serde_json::Value {
    static NULL: serde_json::Value = serde_json::Value::Null;
    fields
        .iter()
        .position(|f| f == name)
        .and_then(|i| row.get(i))
        .unwrap_or(&NULL)
}

fn fmt_val_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn fmt_yi(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                format!("{:.2}", f / 1e8)
            } else {
                String::new()
            }
        }
        serde_json::Value::Null => String::new(),
        _ => fmt_val_str(v),
    }
}

fn fmt_pct(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                format!("{:.2}%", f)
            } else {
                String::new()
            }
        }
        serde_json::Value::Null => String::new(),
        _ => fmt_val_str(v),
    }
}

fn fmt_f2(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                format!("{:.2}", f)
            } else {
                String::new()
            }
        }
        serde_json::Value::Null => String::new(),
        _ => fmt_val_str(v),
    }
}

fn ashare_period(v: &serde_json::Value) -> String {
    let s = fmt_val_str(v);
    // Tushare end_date is yyyymmdd → yyyy-MM
    if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}", &s[..4], &s[4..6])
    } else {
        s
    }
}

async fn fetch_ashare_income(
    http: &Client,
    ts_code: &str,
    periods: usize,
    token: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let fields = "ts_code,end_date,total_revenue,revenue,operate_profit,n_income,n_income_attr_p,basic_eps,diluted_eps,ebit,ebitda";
    let body = tushare_call(
        http,
        token,
        "income_vip",
        ts_code,
        tushare_fetch_limit(periods),
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);
    let items = unique_rows_by_field(items, &field_names, "end_date", periods);

    if items.is_empty() {
        return Ok(format!("[get_financials: no income data for {ts_code}]"));
    }

    let mut out = format!("## {ts_code} · 利润表（近{periods}期）\n\n");
    out.push_str("| 报告期 | 营收（亿） | 营业利润（亿） | 归母净利（亿） | 基本EPS |\n");
    out.push_str("|--------|-----------|--------------|--------------|--------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let revenue = fmt_yi(col(row, &field_names, "total_revenue"));
        let op_profit = fmt_yi(col(row, &field_names, "operate_profit"));
        let net_income = fmt_yi(col(row, &field_names, "n_income_attr_p"));
        let eps = fmt_f2(col(row, &field_names, "basic_eps"));
        out.push_str(&format!(
            "| {period} | {revenue} | {op_profit} | {net_income} | {eps} |\n"
        ));
    }
    out.push_str("\n_来源: Tushare Pro (income_vip)_");
    Ok(out)
}

async fn fetch_ashare_balance(
    http: &Client,
    ts_code: &str,
    periods: usize,
    token: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let fields = "ts_code,end_date,total_assets,total_liab,total_hldr_eqy_exc_min_int,money_cap,accounts_receiv,inventories";
    let body = tushare_call(
        http,
        token,
        "balancesheet",
        ts_code,
        tushare_fetch_limit(periods),
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);
    let items = unique_rows_by_field(items, &field_names, "end_date", periods);

    if items.is_empty() {
        return Ok(format!(
            "[get_financials: no balance sheet data for {ts_code}]"
        ));
    }

    let mut out = format!("## {ts_code} · 资产负债表（近{periods}期）\n\n");
    out.push_str("| 报告期 | 总资产（亿） | 总负债（亿） | 股东权益（亿） | 货币资金（亿） |\n");
    out.push_str("|--------|------------|------------|--------------|-------------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let assets = fmt_yi(col(row, &field_names, "total_assets"));
        let liab = fmt_yi(col(row, &field_names, "total_liab"));
        let equity = fmt_yi(col(row, &field_names, "total_hldr_eqy_exc_min_int"));
        let cash = fmt_yi(col(row, &field_names, "money_cap"));
        out.push_str(&format!(
            "| {period} | {assets} | {liab} | {equity} | {cash} |\n"
        ));
    }
    out.push_str("\n_来源: Tushare Pro (balancesheet)_");
    Ok(out)
}

async fn fetch_ashare_cashflow(
    http: &Client,
    ts_code: &str,
    periods: usize,
    token: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let fields =
        "ts_code,end_date,n_cashflow_act,n_cashflow_inv_act,n_cash_flows_fnc_act,free_cashflow";
    let body = tushare_call(
        http,
        token,
        "cashflow_vip",
        ts_code,
        tushare_fetch_limit(periods),
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);
    let items = unique_rows_by_field(items, &field_names, "end_date", periods);

    if items.is_empty() {
        return Ok(format!("[get_financials: no cashflow data for {ts_code}]"));
    }

    let mut out = format!("## {ts_code} · 现金流量表（近{periods}期）\n\n");
    out.push_str(
        "| 报告期 | 经营活动CF（亿） | 投资活动CF（亿） | 筹资活动CF（亿） | 自由CF（亿） |\n",
    );
    out.push_str("|--------|---------------|---------------|---------------|------------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let oper = fmt_yi(col(row, &field_names, "n_cashflow_act"));
        let inv = fmt_yi(col(row, &field_names, "n_cashflow_inv_act"));
        let fin = fmt_yi(col(row, &field_names, "n_cash_flows_fnc_act"));
        let free = fmt_yi(col(row, &field_names, "free_cashflow"));
        out.push_str(&format!("| {period} | {oper} | {inv} | {fin} | {free} |\n"));
    }
    out.push_str("\n_来源: Tushare Pro (cashflow_vip)_");
    Ok(out)
}

async fn fetch_ashare_ratios(
    http: &Client,
    ts_code: &str,
    periods: usize,
    token: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let fields = "ts_code,end_date,ann_date,eps,bps,roe,roa,grossprofit_margin,debt_to_assets,current_ratio,quick_ratio,revenue_yoy,profit_yoy";
    let body = tushare_call(
        http,
        token,
        "fina_indicator_vip",
        ts_code,
        tushare_fetch_limit(periods),
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);
    let items = unique_rows_by_field(items, &field_names, "end_date", periods);

    if items.is_empty() {
        return Ok(format!("[get_financials: no ratio data for {ts_code}]"));
    }

    let mut out = format!("## {ts_code} · 财务指标（近{periods}期）\n\n");
    out.push_str("| 报告期 | EPS | BPS | ROE% | ROA% | 毛利率% | 资产负债率% | 流动比率 |\n");
    out.push_str("|--------|-----|-----|------|------|--------|-----------|--------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let eps = fmt_f2(col(row, &field_names, "eps"));
        let bps = fmt_f2(col(row, &field_names, "bps"));
        let roe = fmt_pct(col(row, &field_names, "roe"));
        let roa = fmt_pct(col(row, &field_names, "roa"));
        let gpm = fmt_pct(col(row, &field_names, "grossprofit_margin"));
        let d2a = fmt_pct(col(row, &field_names, "debt_to_assets"));
        let cr = fmt_f2(col(row, &field_names, "current_ratio"));
        out.push_str(&format!(
            "| {period} | {eps} | {bps} | {roe} | {roa} | {gpm} | {d2a} | {cr} |\n"
        ));
    }
    out.push_str("\n_来源: Tushare Pro (fina_indicator_vip)_");
    Ok(out)
}

// ── US stock (SEC EDGAR companyfacts) ──────────────────────────────────────
//
// Free, official, no API key required. The companyfacts JSON contains every
// us-gaap fact the registrant has reported, indexed by tag → unit → period.
// We pick a fallback list per metric (Revenues vs. RevenueFromContract... vs.
// SalesRevenueNet, etc.), pull annual (10-K, fp=="FY") rows preferentially,
// and lay them out in the same markdown shape as the old FMP path so the
// agent's prompts don't need to change.

/// Period identifier we use to align metrics across multiple GAAP tags.
///
/// SEC EDGAR's companyfacts payload restates every historical period in
/// every subsequent 10-K filing, so the same `end` date appears repeatedly
/// with **different** `fy` labels — e.g. for NVDA, end=2024-01-28 shows up
/// as `fy=2024` in the FY2024 10-K, `fy=2025` in the FY2025 10-K, and
/// `fy=2026` in the FY2026 10-K. The `fy` field is therefore "reporting
/// year of this filing", not the issuer's true fiscal year. If we keyed on
/// `fy`, we'd produce three rows for the same period.
///
/// The end-date+fp pair is the only stable identifier. Dedup happens at
/// `pick_metric` time: same (end, fp) → keep the value from the most
/// recently filed entry (which carries any post-split / restatement
/// adjustments).
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
struct PeriodKey {
    end: String, // YYYY-MM-DD; sortable lexically
    fp: String,  // "FY" / "Q1" / "Q2" ...
}

impl PeriodKey {
    fn label(&self) -> String {
        if self.fp == "FY" {
            format!("FY ending {}", self.end)
        } else {
            format!("{} ending {}", self.fp, self.end)
        }
    }
}

impl GetFinancialsTool {
    async fn fetch_us_stock_via_sec(
        &self,
        ticker: &str,
        report_type: &str,
        periods: usize,
        cancel: CancellationToken,
    ) -> Result<String> {
        let cik = self.sec_ticker_to_cik(ticker, &cancel).await?;
        let facts = self.sec_companyfacts(cik, &cancel).await?;
        let entity = facts
            .get("entityName")
            .and_then(|v| v.as_str())
            .unwrap_or(ticker)
            .to_string();
        let gaap = facts
            .get("facts")
            .and_then(|f| f.get("us-gaap"))
            .ok_or_else(|| anyhow!("companyfacts payload missing facts.us-gaap (entity {entity})"))?;

        let mut parts: Vec<String> = Vec::new();
        match report_type {
            "income" => parts.push(format_income(&entity, ticker, gaap, periods)),
            "balance" => parts.push(format_balance(&entity, ticker, gaap, periods)),
            "cashflow" => parts.push(format_cashflow(&entity, ticker, gaap, periods)),
            "ratios" => parts.push(format_ratios(&entity, ticker, gaap, periods)),
            "all" => {
                parts.push(format_income(&entity, ticker, gaap, periods));
                parts.push(format_balance(&entity, ticker, gaap, periods));
                parts.push(format_cashflow(&entity, ticker, gaap, periods));
                parts.push(format_ratios(&entity, ticker, gaap, periods));
            }
            _ => bail!("unknown report_type: {report_type}"),
        }
        Ok(parts.join("\n\n"))
    }

    async fn sec_ticker_to_cik(&self, ticker: &str, cancel: &CancellationToken) -> Result<u64> {
        let upper = ticker.to_ascii_uppercase();
        if let Ok(guard) = self.sec_ticker_cache.lock() {
            if let Some(cache) = guard.as_ref() {
                if cache.fetched_at.elapsed() < SEC_TICKER_CACHE_TTL {
                    if let Some(&cik) = cache.map.get(&upper) {
                        return Ok(cik);
                    }
                }
            }
        }
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during SEC ticker map fetch"),
            r = self.sec_http.get(SEC_TICKERS_URL).send() => r?,
        };
        if !resp.status().is_success() {
            bail!(
                "SEC ticker map returned HTTP {} (UA may need to include a contact email)",
                resp.status().as_u16()
            );
        }
        let raw: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during SEC ticker map parse"),
            r = resp.json() => r?,
        };
        let mut map: HashMap<String, u64> = HashMap::new();
        if let Some(obj) = raw.as_object() {
            for (_idx, entry) in obj.iter() {
                let Some(t) = entry.get("ticker").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(cik) = entry.get("cik_str").and_then(|v| v.as_u64()) else {
                    continue;
                };
                map.insert(t.to_ascii_uppercase(), cik);
            }
        }
        let resolved = map
            .get(&upper)
            .copied()
            .ok_or_else(|| anyhow!("unknown US ticker `{ticker}` (not in SEC company list)"))?;
        if let Ok(mut guard) = self.sec_ticker_cache.lock() {
            *guard = Some(SecTickerCache {
                map,
                fetched_at: Instant::now(),
            });
        }
        Ok(resolved)
    }

    async fn sec_companyfacts(
        &self,
        cik: u64,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value> {
        let url = SEC_COMPANYFACTS_URL.replace("{cik}", &format!("{cik:010}"));
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during SEC companyfacts fetch"),
            r = self.sec_http.get(&url).send() => r?,
        };
        if !resp.status().is_success() {
            bail!(
                "SEC companyfacts returned HTTP {} for CIK {cik}",
                resp.status().as_u16()
            );
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during SEC companyfacts parse"),
            r = resp.json() => r?,
        };
        Ok(body)
    }
}

/// Pull the latest `n` annual (or fall-back-to-quarterly if annual is sparse)
/// values for the first GAAP tag in `tags` that exists.
///
/// For each (end, fp) pair we keep only the value from the **most recently
/// filed** entry. This is what eliminates duplicates for issuers that
/// restate prior periods in every new 10-K (e.g. NVDA reports FY2024 data
/// inside the FY2024, FY2025, and FY2026 10-Ks; we want the FY2026 10-K's
/// number because it reflects the 10-for-1 stock split adjustment).
fn pick_metric(
    gaap: &serde_json::Value,
    tags: &[&str],
    units: &[&str],
    periods: usize,
) -> BTreeMap<PeriodKey, f64> {
    /// Insert / update `(end, fp)` to keep the row with the latest `filed`.
    fn ingest(
        arr: &[serde_json::Value],
        annual_only: bool,
        out: &mut BTreeMap<PeriodKey, (String /* filed */, f64)>,
    ) {
        for entry in arr {
            let Some(fp) = entry.get("fp").and_then(|v| v.as_str()) else {
                continue;
            };
            let is_fy = fp == "FY";
            if annual_only && !is_fy {
                continue;
            }
            if !annual_only && is_fy {
                continue;
            }
            let Some(end) = entry.get("end").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(val) = entry.get("val").and_then(|v| v.as_f64()) else {
                continue;
            };
            let filed = entry
                .get("filed")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let key = PeriodKey {
                end: end.to_string(),
                fp: fp.to_string(),
            };
            match out.get(&key) {
                Some((existing_filed, _)) if existing_filed.as_str() >= filed.as_str() => {}
                _ => {
                    out.insert(key, (filed, val));
                }
            }
        }
    }

    for tag in tags {
        let Some(tag_obj) = gaap.get(*tag) else {
            continue;
        };
        let Some(unit_map) = tag_obj.get("units").and_then(|u| u.as_object()) else {
            continue;
        };
        for unit in units {
            let Some(arr) = unit_map.get(*unit).and_then(|v| v.as_array()) else {
                continue;
            };
            let mut staged: BTreeMap<PeriodKey, (String, f64)> = BTreeMap::new();
            ingest(arr, true, &mut staged);
            if staged.len() < periods {
                ingest(arr, false, &mut staged);
            }
            if !staged.is_empty() {
                return staged.into_iter().map(|(k, (_filed, v))| (k, v)).collect();
            }
        }
    }
    BTreeMap::new()
}

fn collect_periods(
    metric_maps: &[&BTreeMap<PeriodKey, f64>],
    periods: usize,
) -> Vec<PeriodKey> {
    let mut all: HashSet<PeriodKey> = HashSet::new();
    for m in metric_maps {
        for k in m.keys() {
            all.insert(k.clone());
        }
    }
    let mut sorted: Vec<PeriodKey> = all.into_iter().collect();
    sorted.sort_by(|a, b| b.end.cmp(&a.end)); // most recent first
    sorted.truncate(periods);
    // Display oldest → newest so the table reads left-to-right chronologically.
    sorted.reverse();
    sorted
}

fn fmt_money(val: Option<&f64>) -> String {
    let Some(&v) = val else {
        return "—".to_string();
    };
    let abs = v.abs();
    if abs >= 1e9 {
        format!("{:.2}B", v / 1e9)
    } else if abs >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if abs >= 1e3 {
        format!("{:.2}K", v / 1e3)
    } else {
        format!("{:.2}", v)
    }
}

fn fmt_per_share(val: Option<&f64>) -> String {
    val.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "—".into())
}

fn fmt_pct_ratio(num: Option<&f64>, den: Option<&f64>) -> String {
    match (num, den) {
        (Some(n), Some(d)) if d.abs() > f64::EPSILON => format!("{:.2}%", (n / d) * 100.0),
        _ => "—".to_string(),
    }
}

fn format_income(
    entity: &str,
    ticker: &str,
    gaap: &serde_json::Value,
    periods: usize,
) -> String {
    let revenue = pick_metric(
        gaap,
        &[
            "Revenues",
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "RevenueFromContractWithCustomerIncludingAssessedTax",
            "SalesRevenueNet",
        ],
        &["USD"],
        periods,
    );
    let gross = pick_metric(gaap, &["GrossProfit"], &["USD"], periods);
    let opinc = pick_metric(gaap, &["OperatingIncomeLoss"], &["USD"], periods);
    let netinc = pick_metric(gaap, &["NetIncomeLoss"], &["USD"], periods);
    let eps = pick_metric(
        gaap,
        &["EarningsPerShareDiluted", "EarningsPerShareBasic"],
        &["USD/shares"],
        periods,
    );

    let rows = collect_periods(&[&revenue, &gross, &opinc, &netinc, &eps], periods);
    if rows.is_empty() {
        return format!("[get_financials: SEC has no income data for {ticker} ({entity})]");
    }
    let mut out = format!("## {ticker} ({entity}) · Income Statement (last {periods} periods)\n\n");
    out.push_str("| Period | Revenue | Gross Profit | Op. Income | Net Income | Diluted EPS |\n");
    out.push_str("|--------|---------|--------------|------------|------------|-------------|\n");
    for k in &rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            k.label(),
            fmt_money(revenue.get(k)),
            fmt_money(gross.get(k)),
            fmt_money(opinc.get(k)),
            fmt_money(netinc.get(k)),
            fmt_per_share(eps.get(k)),
        ));
    }
    out.push_str("\n_Source: SEC EDGAR companyfacts (us-gaap, FY-preferred)_");
    out
}

fn format_balance(
    entity: &str,
    ticker: &str,
    gaap: &serde_json::Value,
    periods: usize,
) -> String {
    let assets = pick_metric(gaap, &["Assets"], &["USD"], periods);
    let liab = pick_metric(gaap, &["Liabilities"], &["USD"], periods);
    let equity = pick_metric(
        gaap,
        &[
            "StockholdersEquity",
            "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        ],
        &["USD"],
        periods,
    );
    let cash = pick_metric(
        gaap,
        &[
            "CashAndCashEquivalentsAtCarryingValue",
            "CashCashEquivalentsRestrictedCashAndRestrictedCashEquivalents",
        ],
        &["USD"],
        periods,
    );
    let lt_debt = pick_metric(
        gaap,
        &["LongTermDebtNoncurrent", "LongTermDebt"],
        &["USD"],
        periods,
    );

    let rows = collect_periods(&[&assets, &liab, &equity, &cash, &lt_debt], periods);
    if rows.is_empty() {
        return format!(
            "[get_financials: SEC has no balance-sheet data for {ticker} ({entity})]"
        );
    }
    let mut out = format!("## {ticker} ({entity}) · Balance Sheet (last {periods} periods)\n\n");
    out.push_str("| Period | Total Assets | Total Liab. | Equity | Cash | LT Debt |\n");
    out.push_str("|--------|--------------|-------------|--------|------|---------|\n");
    for k in &rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            k.label(),
            fmt_money(assets.get(k)),
            fmt_money(liab.get(k)),
            fmt_money(equity.get(k)),
            fmt_money(cash.get(k)),
            fmt_money(lt_debt.get(k)),
        ));
    }
    out.push_str("\n_Source: SEC EDGAR companyfacts (us-gaap, FY-preferred)_");
    out
}

fn format_cashflow(
    entity: &str,
    ticker: &str,
    gaap: &serde_json::Value,
    periods: usize,
) -> String {
    let ocf = pick_metric(
        gaap,
        &["NetCashProvidedByUsedInOperatingActivities"],
        &["USD"],
        periods,
    );
    let icf = pick_metric(
        gaap,
        &["NetCashProvidedByUsedInInvestingActivities"],
        &["USD"],
        periods,
    );
    let fcf = pick_metric(
        gaap,
        &["NetCashProvidedByUsedInFinancingActivities"],
        &["USD"],
        periods,
    );
    let capex = pick_metric(
        gaap,
        &["PaymentsToAcquirePropertyPlantAndEquipment"],
        &["USD"],
        periods,
    );

    let rows = collect_periods(&[&ocf, &icf, &fcf, &capex], periods);
    if rows.is_empty() {
        return format!("[get_financials: SEC has no cash-flow data for {ticker} ({entity})]");
    }
    let mut out = format!("## {ticker} ({entity}) · Cash Flow (last {periods} periods)\n\n");
    out.push_str("| Period | Operating CF | Investing CF | Financing CF | CapEx | FCF (OCF-CapEx) |\n");
    out.push_str("|--------|--------------|--------------|--------------|-------|-----------------|\n");
    for k in &rows {
        let free = match (ocf.get(k), capex.get(k)) {
            (Some(o), Some(c)) => Some(*o - c.abs()),
            (Some(o), None) => Some(*o),
            _ => None,
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            k.label(),
            fmt_money(ocf.get(k)),
            fmt_money(icf.get(k)),
            fmt_money(fcf.get(k)),
            fmt_money(capex.get(k)),
            fmt_money(free.as_ref()),
        ));
    }
    out.push_str("\n_Source: SEC EDGAR companyfacts (us-gaap, FY-preferred)_");
    out
}

fn format_ratios(
    entity: &str,
    ticker: &str,
    gaap: &serde_json::Value,
    periods: usize,
) -> String {
    let revenue = pick_metric(
        gaap,
        &[
            "Revenues",
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "RevenueFromContractWithCustomerIncludingAssessedTax",
            "SalesRevenueNet",
        ],
        &["USD"],
        periods,
    );
    let gross = pick_metric(gaap, &["GrossProfit"], &["USD"], periods);
    let opinc = pick_metric(gaap, &["OperatingIncomeLoss"], &["USD"], periods);
    let netinc = pick_metric(gaap, &["NetIncomeLoss"], &["USD"], periods);
    let assets = pick_metric(gaap, &["Assets"], &["USD"], periods);
    let liab = pick_metric(gaap, &["Liabilities"], &["USD"], periods);
    let equity = pick_metric(
        gaap,
        &[
            "StockholdersEquity",
            "StockholdersEquityIncludingPortionAttributableToNoncontrollingInterest",
        ],
        &["USD"],
        periods,
    );

    let rows = collect_periods(
        &[&revenue, &gross, &opinc, &netinc, &assets, &liab, &equity],
        periods,
    );
    if rows.is_empty() {
        return format!("[get_financials: SEC has no ratio inputs for {ticker} ({entity})]");
    }
    let mut out = format!("## {ticker} ({entity}) · Key Ratios (last {periods} periods)\n\n");
    out.push_str("| Period | Gross Margin | Op. Margin | Net Margin | ROE | ROA | Debt/Equity |\n");
    out.push_str("|--------|--------------|------------|------------|-----|-----|-------------|\n");
    for k in &rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            k.label(),
            fmt_pct_ratio(gross.get(k), revenue.get(k)),
            fmt_pct_ratio(opinc.get(k), revenue.get(k)),
            fmt_pct_ratio(netinc.get(k), revenue.get(k)),
            fmt_pct_ratio(netinc.get(k), equity.get(k)),
            fmt_pct_ratio(netinc.get(k), assets.get(k)),
            match (liab.get(k), equity.get(k)) {
                (Some(l), Some(e)) if e.abs() > f64::EPSILON => format!("{:.2}", l / e),
                _ => "—".to_string(),
            },
        ));
    }
    out.push_str("\n_Source: SEC EDGAR companyfacts (derived from us-gaap; market multiples like P/E, P/B not provided — combine with `market_quote` and your own EPS/Book inputs)._");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_metric_prefers_fy_and_falls_back_to_quarterly() {
        let payload = serde_json::json!({
            "Revenues": {
                "units": {
                    "USD": [
                        {"end": "2023-01-29", "val": 26974000000_i64, "fy": 2023, "fp": "FY", "filed": "2023-02-24"},
                        {"end": "2023-04-30", "val": 7192000000_i64, "fy": 2024, "fp": "Q1", "filed": "2023-05-25"},
                        {"end": "2024-01-28", "val": 60922000000_i64, "fy": 2024, "fp": "FY", "filed": "2024-02-21"},
                    ]
                }
            }
        });
        let map = pick_metric(&payload, &["Revenues"], &["USD"], 4);
        // FY entries alone count as 2; we ask for 4, so the Q1 should be
        // pulled in too.
        assert_eq!(map.len(), 3);
        let labels: Vec<String> = map.keys().map(|k| k.label()).collect();
        assert!(labels.iter().any(|l| l.contains("2024-01-28")));
        assert!(labels.iter().any(|l| l.contains("Q1 ending 2023-04-30")));
    }

    #[test]
    fn pick_metric_dedups_restated_periods_keeping_latest_filed() {
        // Same end+FY across three 10-K filings — the issuer restated
        // historical numbers each time. We must keep only the value from
        // the most recently filed 10-K (post-split, post-restatement).
        let payload = serde_json::json!({
            "EarningsPerShareDiluted": {
                "units": {
                    "USD/shares": [
                        {"end": "2024-01-28", "val": 11.93, "fy": 2024, "fp": "FY", "filed": "2024-02-21"},
                        {"end": "2024-01-28", "val": 1.19,  "fy": 2025, "fp": "FY", "filed": "2025-02-26"},
                        {"end": "2024-01-28", "val": 1.19,  "fy": 2026, "fp": "FY", "filed": "2026-02-25"},
                    ]
                }
            }
        });
        let map = pick_metric(
            &payload,
            &["EarningsPerShareDiluted"],
            &["USD/shares"],
            4,
        );
        // One row, not three — same (end, fp) collapses.
        assert_eq!(map.len(), 1);
        let (_k, v) = map.iter().next().unwrap();
        // Latest-filed value is 1.19 (post-split adjusted).
        assert!((v - 1.19).abs() < 1e-9, "expected 1.19, got {v}");
    }

    #[test]
    fn pick_metric_fallback_to_secondary_tag() {
        let payload = serde_json::json!({
            "RevenueFromContractWithCustomerExcludingAssessedTax": {
                "units": {
                    "USD": [
                        {"end": "2024-12-31", "val": 1000000000_i64, "fy": 2024, "fp": "FY", "filed": "2025-02-15"},
                    ]
                }
            }
        });
        let map = pick_metric(
            &payload,
            &["Revenues", "RevenueFromContractWithCustomerExcludingAssessedTax"],
            &["USD"],
            2,
        );
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn fmt_money_scales_correctly() {
        assert_eq!(fmt_money(Some(&1_500_000_000.0)), "1.50B");
        assert_eq!(fmt_money(Some(&12_345_000.0)), "12.35M");
        assert_eq!(fmt_money(Some(&500.0)), "500.00");
        assert_eq!(fmt_money(None), "—");
    }
}
