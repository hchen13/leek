use std::{collections::HashSet, time::Duration};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::{data_provider_tokens, ToolContext, ToolHandler};

const TOOL_NAME: &str = "get_financials";
const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";
const FMP_BASE: &str = "https://financialmodelingprep.com/stable";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_PERIODS: u64 = 4;
const MAX_PERIODS: u64 = 24;

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
                Covers A-shares and US stocks through configured financial-data sources.\n\
                For A-shares, use this as a structured financial-data aid; for final \
                company financial claims, prefer announcements, exchange/CNINFO disclosures, \
                and company IR pages when available. Output includes field metadata; reconcile \
                announcement/tool conflicts by report period, scope, unit, and field definition \
                before mixing numbers.\n\
                - A-share ts_code: \"600519.SH\", \"000001.SZ\"\n\
                - US ticker: \"AAPL\", \"TSLA\", \"NVDA\"\n\
                report_type controls what to fetch:\n\
                - \"income\": income statement (revenue, profit, EPS)\n\
                - \"balance\": balance sheet (assets, liabilities, equity)\n\
                - \"cashflow\": cash flow statement\n\
                - \"ratios\": key financial ratios (ROE, ROA, PE, PB, margins, debt ratio)\n\
                - \"all\": fetch income + balance sheet + cash flow + ratios\n\
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
        ctx: &ToolContext,
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
            "a_share" => {
                fetch_a_share(&self.http, ctx, &ticker, report_type, periods, cancel).await
            }
            "us_stock" => fetch_us_stock(&self.http, &ticker, report_type, periods, cancel).await,
            _ => bail!("unknown market: {market}"),
        }
    }
}

// ── A-share (Tushare) ──────────────────────────────────────────────────────

async fn fetch_a_share(
    http: &Client,
    ctx: &ToolContext,
    ts_code: &str,
    report_type: &str,
    periods: usize,
    cancel: CancellationToken,
) -> Result<String> {
    let token = data_provider_tokens::tushare_token(ctx).await?;

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
        "dividends" => {
            parts.push(fetch_ashare_dividend(http, ts_code, periods, &token, &cancel).await?);
        }
        "all" => {
            parts.push(fetch_ashare_income(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_balance(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_cashflow(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_ratios(http, ts_code, periods, &token, &cancel).await?);
            parts.push(fetch_ashare_dividend(http, ts_code, periods, &token, &cancel).await?);
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
    let fields = "ts_code,end_date,total_revenue,revenue,oper_cost,operate_profit,total_profit,n_income,n_income_attr_p,basic_eps,diluted_eps,ebit,ebitda";
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
    out.push_str("| 报告期 | 营业总收入（亿） | 营业收入（亿） | 营业成本（亿） | 营业利润（亿） | 利润总额（亿） | 净利润（亿） | 归母净利（亿） | 基本EPS | 稀释EPS | EBIT（亿） | EBITDA（亿） |\n");
    out.push_str("|--------|---------------|-------------|-------------|--------------|--------------|------------|--------------|--------|--------|----------|------------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let total_revenue = fmt_yi(col(row, &field_names, "total_revenue"));
        let revenue = fmt_yi(col(row, &field_names, "revenue"));
        let oper_cost = fmt_yi(col(row, &field_names, "oper_cost"));
        let op_profit = fmt_yi(col(row, &field_names, "operate_profit"));
        let total_profit = fmt_yi(col(row, &field_names, "total_profit"));
        let net_profit = fmt_yi(col(row, &field_names, "n_income"));
        let net_income = fmt_yi(col(row, &field_names, "n_income_attr_p"));
        let eps = fmt_f2(col(row, &field_names, "basic_eps"));
        let diluted_eps = fmt_f2(col(row, &field_names, "diluted_eps"));
        let ebit = fmt_yi(col(row, &field_names, "ebit"));
        let ebitda = fmt_yi(col(row, &field_names, "ebitda"));
        out.push_str(&format!(
            "| {period} | {total_revenue} | {revenue} | {oper_cost} | {op_profit} | {total_profit} | {net_profit} | {net_income} | {eps} | {diluted_eps} | {ebit} | {ebitda} |\n"
        ));
    }
    append_ashare_income_metadata(&mut out);
    Ok(out)
}

fn append_ashare_income_metadata(out: &mut String) {
    out.push_str("\n_来源: Tushare Pro (income_vip)_\n");
    out.push_str("_字段口径: total_revenue=营业总收入；revenue=营业收入；operate_profit=营业利润；n_income=净利润；n_income_attr_p=归母净利润；basic_eps=基本每股收益。金额字段来自 Tushare 原始元单位并换算为亿元。公告/交易所/巨潮/公司官网与工具数值冲突时，必须先 reconcile 报告期、合并范围、单位和字段口径，不得把营业收入、营业总收入、净利润、归母净利润直接混用。_");
}

async fn fetch_ashare_balance(
    http: &Client,
    ts_code: &str,
    periods: usize,
    token: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let fields = "ts_code,end_date,total_assets,total_cur_assets,total_liab,total_cur_liab,total_hldr_eqy_exc_min_int,money_cap,accounts_receiv,inventories,fix_assets,goodwill";
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
    out.push_str("| 报告期 | 总资产（亿） | 流动资产（亿） | 总负债（亿） | 流动负债（亿） | 股东权益（亿） | 货币资金（亿） | 应收账款（亿） | 存货（亿） | 固定资产（亿） | 商誉（亿） |\n");
    out.push_str("|--------|------------|-------------|------------|-------------|--------------|-------------|-------------|----------|-------------|----------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let assets = fmt_yi(col(row, &field_names, "total_assets"));
        let cur_assets = fmt_yi(col(row, &field_names, "total_cur_assets"));
        let liab = fmt_yi(col(row, &field_names, "total_liab"));
        let cur_liab = fmt_yi(col(row, &field_names, "total_cur_liab"));
        let equity = fmt_yi(col(row, &field_names, "total_hldr_eqy_exc_min_int"));
        let cash = fmt_yi(col(row, &field_names, "money_cap"));
        let ar = fmt_yi(col(row, &field_names, "accounts_receiv"));
        let inv = fmt_yi(col(row, &field_names, "inventories"));
        let fix = fmt_yi(col(row, &field_names, "fix_assets"));
        let goodwill = fmt_yi(col(row, &field_names, "goodwill"));
        out.push_str(&format!(
            "| {period} | {assets} | {cur_assets} | {liab} | {cur_liab} | {equity} | {cash} | {ar} | {inv} | {fix} | {goodwill} |\n"
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
        "ts_code,end_date,n_cashflow_act,n_cashflow_inv_act,n_cash_flows_fnc_act,free_cashflow,c_pay_acq_const_fiolta,n_incr_cash_cash_equ";
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
        "| 报告期 | 经营活动CF（亿） | 投资活动CF（亿） | 筹资活动CF（亿） | 自由CF（亿） | 资本开支（亿） | 现金净增加（亿） |\n",
    );
    out.push_str("|--------|---------------|---------------|---------------|------------|-------------|---------------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let oper = fmt_yi(col(row, &field_names, "n_cashflow_act"));
        let inv = fmt_yi(col(row, &field_names, "n_cashflow_inv_act"));
        let fin = fmt_yi(col(row, &field_names, "n_cash_flows_fnc_act"));
        let free = fmt_yi(col(row, &field_names, "free_cashflow"));
        let capex = fmt_yi(col(row, &field_names, "c_pay_acq_const_fiolta"));
        let net_incr = fmt_yi(col(row, &field_names, "n_incr_cash_cash_equ"));
        out.push_str(&format!(
            "| {period} | {oper} | {inv} | {fin} | {free} | {capex} | {net_incr} |\n"
        ));
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
    let fields = "ts_code,end_date,ann_date,eps,bps,roe,roa,netprofit_margin,grossprofit_margin,debt_to_assets,assets_turn,current_ratio,quick_ratio,or_yoy,netprofit_yoy";
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
    out.push_str("| 报告期 | EPS | BPS | ROE% | ROA% | 净利率% | 毛利率% | 资产负债率% | 总资产周转 | 流动比率 | 速动比率 | 营收同比% | 净利同比% |\n");
    out.push_str("|--------|-----|-----|------|------|--------|--------|-----------|----------|--------|--------|---------|---------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let eps = fmt_f2(col(row, &field_names, "eps"));
        let bps = fmt_f2(col(row, &field_names, "bps"));
        let roe = fmt_pct(col(row, &field_names, "roe"));
        let roa = fmt_pct(col(row, &field_names, "roa"));
        let npm = fmt_pct(col(row, &field_names, "netprofit_margin"));
        let gpm = fmt_pct(col(row, &field_names, "grossprofit_margin"));
        let d2a = fmt_pct(col(row, &field_names, "debt_to_assets"));
        let at = fmt_f2(col(row, &field_names, "assets_turn"));
        let cr = fmt_f2(col(row, &field_names, "current_ratio"));
        let qr = fmt_f2(col(row, &field_names, "quick_ratio"));
        let ry = fmt_pct(col(row, &field_names, "or_yoy"));
        let py = fmt_pct(col(row, &field_names, "netprofit_yoy"));
        out.push_str(&format!(
            "| {period} | {eps} | {bps} | {roe} | {roa} | {npm} | {gpm} | {d2a} | {at} | {cr} | {qr} | {ry} | {py} |\n"
        ));
    }
    out.push_str("\n_来源: Tushare Pro (fina_indicator_vip)_");
    Ok(out)
}

async fn fetch_ashare_dividend(
    http: &Client,
    ts_code: &str,
    periods: usize,
    token: &str,
    cancel: &CancellationToken,
) -> Result<String> {
    let fields = "ts_code,end_date,ann_date,div_proc,stk_div,cash_div,cash_div_tax";
    // `dividend` returns one row per announcement stage (预案 / 股东大会通过 / 实施);
    // fetch a wider window, keep only implemented payouts, dedup by report period.
    let body = tushare_call(
        http,
        token,
        "dividend",
        ts_code,
        periods * 4 + 24,
        fields,
        cancel,
    )
    .await?;
    let (field_names, items) = tushare_extract(&body);
    let implemented: Vec<Vec<serde_json::Value>> = items
        .into_iter()
        .filter(|row| {
            col(row, &field_names, "div_proc")
                .as_str()
                .map(|s| s.contains("实施"))
                .unwrap_or(false)
        })
        .collect();
    let items = unique_rows_by_field(implemented, &field_names, "end_date", periods);

    if items.is_empty() {
        return Ok(format!(
            "## {ts_code} · 分红\n\n[get_financials: 该标的近年无已实施分红记录（或 Tushare 无数据）]"
        ));
    }

    let mut out = format!("## {ts_code} · 分红（近{periods}期已实施）\n\n");
    out.push_str("| 报告期 | 每股现金分红税前（元） | 每股现金分红税后（元） | 每股送转（股） |\n");
    out.push_str("|--------|--------------------|--------------------|--------------|\n");
    for row in &items {
        let period = ashare_period(col(row, &field_names, "end_date"));
        let pre = fmt_f2(col(row, &field_names, "cash_div_tax"));
        let post = fmt_f2(col(row, &field_names, "cash_div"));
        let stk = fmt_f2(col(row, &field_names, "stk_div"));
        out.push_str(&format!("| {period} | {pre} | {post} | {stk} |\n"));
    }
    out.push_str("\n_来源: Tushare Pro (dividend)；仅含 div_proc=实施 的记录，按报告期去重_");
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
            return Ok("[get_financials: US stock financial-data source is not configured. Treat this as a blocked source, not as missing company financials.]"
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
            parts.push(fetch_fmp_balance(http, ticker, periods, &key, &cancel).await?);
            parts.push(fetch_fmp_cashflow(http, ticker, periods, &key, &cancel).await?);
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
        let status = resp.status().as_u16();
        let body = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = resp.text() => r.unwrap_or_default(),
        };
        bail!("{}", fmp_http_error(status, &body));
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

fn fmp_http_error(status: u16, body: &str) -> String {
    let base = match status {
        401 | 403 => {
            format!("FMP HTTP {status}: provider authentication or endpoint permission denied.")
        }
        402 => {
            "FMP HTTP 402: provider access/quota does not allow this financial-statement endpoint."
                .to_string()
        }
        429 => {
            "FMP HTTP 429: provider rate limit exceeded. Retry later or lower request frequency."
                .to_string()
        }
        _ => format!("FMP HTTP {status}: provider request failed."),
    };
    let detail = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.is_empty() {
        base
    } else {
        format!(
            "{base} Provider response: {}",
            truncate_error_detail(&detail, 180)
        )
    }
}

fn truncate_error_detail(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in s.chars().take(max_chars) {
        out.push(ch);
    }
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmp_http_402_mentions_provider_access_quota() {
        let msg = fmp_http_error(402, "{\"Error Message\":\"Plan limit\"}");
        assert!(msg.contains("access/quota"));
        assert!(msg.contains("Plan limit"));
    }

    #[test]
    fn ashare_income_metadata_names_financial_statement_fields() {
        let mut out = String::new();
        append_ashare_income_metadata(&mut out);
        assert!(out.contains("total_revenue=营业总收入"));
        assert!(out.contains("revenue=营业收入"));
        assert!(out.contains("n_income=净利润"));
        assert!(out.contains("n_income_attr_p=归母净利润"));
        assert!(out.contains("不得把营业收入、营业总收入、净利润、归母净利润直接混用"));
    }

    #[test]
    fn tool_description_requires_reconcile_on_ashare_conflicts() {
        let tool = GetFinancialsTool::new().unwrap();
        let description = match tool.spec() {
            ToolSpec::Function { description, .. } => description,
            _ => panic!("get_financials should expose a function tool"),
        };
        assert!(description.contains("exchange/CNINFO"));
        assert!(description.contains("field metadata"));
        assert!(description.contains("reconcile"));
    }
}
