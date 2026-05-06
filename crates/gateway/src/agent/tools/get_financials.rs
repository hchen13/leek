use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_financials";
const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";
const FMP_BASE: &str = "https://financialmodelingprep.com/stable";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_PERIODS: u64 = 4;
const MAX_PERIODS: u64 = 8;

pub struct GetFinancialsTool {
    http: Client,
}

impl GetFinancialsTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
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
                Covers A-shares (via Tushare Pro) and US stocks (via FMP).\n\
                - A-share ts_code: \"600519.SH\", \"000001.SZ\"\n\
                - US ticker: \"AAPL\", \"TSLA\", \"NVDA\"\n\
                report_type controls what to fetch:\n\
                - \"income\": income statement (revenue, profit, EPS)\n\
                - \"balance\": balance sheet (assets, liabilities, equity)\n\
                - \"cashflow\": cash flow statement\n\
                - \"ratios\": key financial ratios (ROE, ROA, PE, PB, margins, debt ratio)\n\
                - \"all\": fetch income + ratios together (most useful for quick analysis)\n\
                Returns markdown tables with the most recent N periods."
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
            "us_stock" => fetch_us_stock(&self.http, &ticker, report_type, periods, cancel).await,
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
    let body = tushare_call(http, token, "income_vip", ts_code, periods, fields, cancel).await?;
    let (field_names, items) = tushare_extract(&body);

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
        periods,
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);

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
    let fields = "ts_code,end_date,net_operate_cashflow,n_cashflow_inv_act,n_cash_flows_fnc_act,free_cashflow";
    let body = tushare_call(
        http,
        token,
        "cashflow_vip",
        ts_code,
        periods,
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);

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
        let oper = fmt_yi(col(row, &field_names, "net_operate_cashflow"));
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
        periods,
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);

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

// ── US stock (FMP) ─────────────────────────────────────────────────────────

async fn fetch_us_stock(
    http: &Client,
    ticker: &str,
    report_type: &str,
    periods: usize,
    cancel: CancellationToken,
) -> Result<String> {
    let key = match std::env::var("FMP_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            return Ok("[get_financials: FMP_API_KEY not set — get a free key at \
                 https://financialmodelingprep.com/developer/docs to access US stock financials]"
                .to_string());
        }
    };

    let mut parts: Vec<String> = Vec::new();

    match report_type {
        "income" => {
            parts.push(fetch_fmp_income(http, ticker, periods, &key, &cancel).await?);
        }
        "balance" => {
            parts.push(fetch_fmp_balance(http, ticker, periods, &key, &cancel).await?);
        }
        "cashflow" => {
            parts.push(fetch_fmp_cashflow(http, ticker, periods, &key, &cancel).await?);
        }
        "ratios" => {
            parts.push(fetch_fmp_ratios(http, ticker, periods, &key, &cancel).await?);
        }
        "all" => {
            parts.push(fetch_fmp_income(http, ticker, periods, &key, &cancel).await?);
            parts.push(fetch_fmp_ratios(http, ticker, periods, &key, &cancel).await?);
        }
        _ => bail!("unknown report_type: {report_type}"),
    }

    Ok(parts.join("\n\n"))
}

async fn fmp_get(
    http: &Client,
    url: &str,
    cancel: &CancellationToken,
) -> Result<Vec<serde_json::Value>> {
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("aborted"),
        r = http.get(url).send() => r?,
    };
    if !resp.status().is_success() {
        bail!("FMP HTTP {}", resp.status().as_u16());
    }
    let body: serde_json::Value = tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("aborted"),
        r = resp.json() => r?,
    };
    match body {
        serde_json::Value::Array(arr) => Ok(arr),
        other => bail!("FMP unexpected response shape: {}", other),
    }
}

fn fmp_str(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn fmt_b(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_f64())
        .map(|f| {
            let b = f / 1e9;
            if b.abs() >= 1.0 {
                format!("{:.2}B", b)
            } else {
                format!("{:.0}M", f / 1e6)
            }
        })
        .unwrap_or_default()
}

fn fmt_ratio_pct(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_f64())
        .map(|f| format!("{:.2}%", f * 100.0))
        .unwrap_or_default()
}

fn fmt_ratio(row: &serde_json::Value, key: &str) -> String {
    row.get(key)
        .and_then(|v| v.as_f64())
        .map(|f| format!("{:.2}", f))
        .unwrap_or_default()
}

async fn fetch_fmp_income(
    http: &Client,
    ticker: &str,
    periods: usize,
    key: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let url = format!("{FMP_BASE}/income-statement?symbol={ticker}&limit={periods}&apikey={key}");
    let rows = fmp_get(http, &url, cancel).await?;

    if rows.is_empty() {
        return Ok(format!("[get_financials: no income data for {ticker}]"));
    }

    let mut out = format!("## {ticker} · Income Statement (last {periods} periods)\n\n");
    out.push_str("| Date | Revenue | Gross Profit | Op. Income | Net Income | EPS |\n");
    out.push_str("|------|---------|-------------|-----------|-----------|-----|\n");
    for row in &rows {
        let date = fmp_str(row, "date");
        let revenue = fmt_b(row, "revenue");
        let gross = fmt_b(row, "grossProfit");
        let op_inc = fmt_b(row, "operatingIncome");
        let net = fmt_b(row, "netIncome");
        let eps = fmt_ratio(row, "eps");
        out.push_str(&format!(
            "| {date} | {revenue} | {gross} | {op_inc} | {net} | {eps} |\n"
        ));
    }
    out.push_str("\n_Source: Financial Modeling Prep (income-statement)_");
    Ok(out)
}

async fn fetch_fmp_balance(
    http: &Client,
    ticker: &str,
    periods: usize,
    key: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let url =
        format!("{FMP_BASE}/balance-sheet-statement?symbol={ticker}&limit={periods}&apikey={key}");
    let rows = fmp_get(http, &url, cancel).await?;

    if rows.is_empty() {
        return Ok(format!(
            "[get_financials: no balance sheet data for {ticker}]"
        ));
    }

    let mut out = format!("## {ticker} · Balance Sheet (last {periods} periods)\n\n");
    out.push_str("| Date | Total Assets | Total Liabilities | Stockholders' Equity | Cash |\n");
    out.push_str("|------|-------------|------------------|---------------------|------|\n");
    for row in &rows {
        let date = fmp_str(row, "date");
        let assets = fmt_b(row, "totalAssets");
        let liab = fmt_b(row, "totalLiabilities");
        let equity = fmt_b(row, "totalStockholdersEquity");
        let cash = fmt_b(row, "cashAndShortTermInvestments");
        out.push_str(&format!(
            "| {date} | {assets} | {liab} | {equity} | {cash} |\n"
        ));
    }
    out.push_str("\n_Source: Financial Modeling Prep (balance-sheet-statement)_");
    Ok(out)
}

async fn fetch_fmp_cashflow(
    http: &Client,
    ticker: &str,
    periods: usize,
    key: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let url =
        format!("{FMP_BASE}/cash-flow-statement?symbol={ticker}&limit={periods}&apikey={key}");
    let rows = fmp_get(http, &url, cancel).await?;

    if rows.is_empty() {
        return Ok(format!("[get_financials: no cashflow data for {ticker}]"));
    }

    let mut out = format!("## {ticker} · Cash Flow Statement (last {periods} periods)\n\n");
    out.push_str("| Date | Operating CF | Investing CF | Financing CF | Free CF |\n");
    out.push_str("|------|-------------|-------------|-------------|--------|\n");
    for row in &rows {
        let date = fmp_str(row, "date");
        let oper = fmt_b(row, "operatingCashFlow");
        let inv = fmt_b(row, "investingActivitiesCashFlow");
        let fin = fmt_b(row, "financingActivitiesCashFlow");
        let free = fmt_b(row, "freeCashFlow");
        out.push_str(&format!("| {date} | {oper} | {inv} | {fin} | {free} |\n"));
    }
    out.push_str("\n_Source: Financial Modeling Prep (cash-flow-statement)_");
    Ok(out)
}

async fn fetch_fmp_ratios(
    http: &Client,
    ticker: &str,
    periods: usize,
    key: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let url = format!("{FMP_BASE}/ratios?symbol={ticker}&limit={periods}&apikey={key}");
    let rows = fmp_get(http, &url, cancel).await?;
    let metrics_url =
        format!("{FMP_BASE}/key-metrics?symbol={ticker}&limit={periods}&apikey={key}");
    let metrics = fmp_get(http, &metrics_url, cancel)
        .await
        .unwrap_or_default();

    if rows.is_empty() {
        return Ok(format!("[get_financials: no ratio data for {ticker}]"));
    }

    let mut out = format!("## {ticker} · Key Ratios (last {periods} periods)\n\n");
    out.push_str(
        "| Date | ROE% | ROA% | Gross Margin% | Debt/Equity | Current Ratio | P/E | P/B |\n",
    );
    out.push_str("|------|------|------|--------------|------------|--------------|-----|-----|\n");
    for row in &rows {
        let date = fmp_str(row, "date");
        let metric = metrics
            .iter()
            .find(|m| fmp_str(m, "date") == date)
            .unwrap_or(row);
        let roe = fmt_ratio_pct(metric, "returnOnEquity");
        let roa = fmt_ratio_pct(metric, "returnOnAssets");
        let gpm = fmt_ratio_pct(row, "grossProfitMargin");
        let d2e = fmt_ratio(row, "debtToEquityRatio");
        let cr = fmt_ratio(row, "currentRatio");
        let pe = fmt_ratio(row, "priceToEarningsRatio");
        let pb = fmt_ratio(row, "priceToBookRatio");
        out.push_str(&format!(
            "| {date} | {roe} | {roa} | {gpm} | {d2e} | {cr} | {pe} | {pb} |\n"
        ));
    }
    out.push_str("\n_Source: Financial Modeling Prep (ratios)_");
    Ok(out)
}
