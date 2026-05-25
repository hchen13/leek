use std::time::Duration;

use anyhow::{Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{ToolContext, ToolHandler, data_provider_tokens};

const TOOL_NAME: &str = "get_china_fund_context";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 30;

pub struct GetChinaFundContextTool {
    http: Client,
}

impl GetChinaFundContextTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
    }

    async fn tushare_post(
        &self,
        token: &str,
        api_name: &str,
        params: serde_json::Value,
        fields: &str,
        cancel: &CancellationToken,
    ) -> Result<Table> {
        let client = TushareClient::with_client(token, self.http.clone())?;
        let response = client
            .query_cancelled(api_name, params, fields, cancel)
            .await?;
        Ok(Table::from_response(response))
    }
}

#[async_trait]
impl ToolHandler for GetChinaFundContextTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Build China ETF/fund context from configured fund-market data sources: fund profile, exchange quote for listed ETF/LOF, NAV, portfolio holdings, fund share, manager, and dividend history. Use this for A-share ETF/fund research instead of stitching many raw endpoints. Empty sections are valid data gaps; do not treat them as proof."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "fund_code": {
                        "type": "string",
                        "description": "Optional fund code, e.g. 510300.SH for ETF or 110022.OF for open-end fund. Without it, returns a fund universe sample for the selected market."
                    },
                    "market": {
                        "type": "string",
                        "enum": ["E", "O", "C"],
                        "description": "Fund market: E=exchange traded ETF/LOF, O=open-end fund, C=closed-end fund. Default inferred from fund_code, otherwise E."
                    },
                    "sections": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["profile", "quote", "nav", "portfolio", "share", "manager", "dividend"]
                        },
                        "description": "Default with fund_code: profile, quote, nav, portfolio, share, manager, dividend. Without fund_code: profile only."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Rows per section, default 8, max 30."
                    }
                }
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let token = data_provider_tokens::tushare_token(ctx).await?;
        let fund_code = args
            .get("fund_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let market = args
            .get("market")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_uppercase)
            .unwrap_or_else(|| infer_market(fund_code.as_deref()).to_string());
        let limit = limit(&args);
        let sections = sections(&args, fund_code.is_some());

        if fund_code.is_none() && sections.iter().any(|s| s != "profile") {
            bail!("fund_code is required for quote/nav/portfolio/share/manager/dividend");
        }

        let mut out = match fund_code.as_deref() {
            Some(code) => format!("## 中国基金上下文 · {code}\n\n"),
            None => format!("## 中国基金池样本 · market={market}\n\n"),
        };

        if sections.iter().any(|s| s == "profile") {
            let profile = self
                .tushare_post(
                    &token,
                    "fund_basic",
                    fund_basic_params(fund_code.as_deref(), &market),
                    "ts_code,name,management,custodian,fund_type,found_date,list_date,issue_amount,m_fee,c_fee,status,benchmark,invest_type,type,market",
                    &cancel,
                )
                .await?;
            out.push_str(&render_profile(&profile, limit));
        }

        if let Some(code) = fund_code.as_deref() {
            if sections.iter().any(|s| s == "quote") {
                let quote = self
                    .tushare_post(
                        &token,
                        "fund_daily",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,trade_date,open,high,low,close,pre_close,change,pct_chg,vol,amount",
                        &cancel,
                    )
                    .await?;
                out.push_str(&render_quote(&quote));
            }

            if sections.iter().any(|s| s == "nav") {
                let nav = self
                    .tushare_post(
                        &token,
                        "fund_nav",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,ann_date,unit_nav,accum_nav,net_asset,total_netasset,adj_nav",
                        &cancel,
                    )
                    .await?;
                out.push_str(&render_nav(&nav));
            }

            if sections.iter().any(|s| s == "portfolio") {
                let portfolio = self
                    .tushare_post(
                        &token,
                        "fund_portfolio",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,ann_date,end_date,symbol,mkv,amount,stk_mkv_ratio,stk_float_ratio",
                        &cancel,
                    )
                    .await?;
                out.push_str(&render_portfolio(&portfolio));
            }

            if sections.iter().any(|s| s == "share") {
                let share = self
                    .tushare_post(
                        &token,
                        "fund_share",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,trade_date,fd_share",
                        &cancel,
                    )
                    .await?;
                out.push_str(&render_share(&share));
            }

            if sections.iter().any(|s| s == "manager") {
                let manager = self
                    .tushare_post(
                        &token,
                        "fund_manager",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,ann_date,name,gender,birth_year,edu,nationality,begin_date,end_date,resume",
                        &cancel,
                    )
                    .await?;
                out.push_str(&render_manager(&manager));
            }

            if sections.iter().any(|s| s == "dividend") {
                let dividend = self
                    .tushare_post(
                        &token,
                        "fund_div",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,ann_date,imp_anndate,base_date,div_proc,record_date,ex_date,pay_date,div_cash,base_unit,base_year",
                        &cancel,
                    )
                    .await?;
                out.push_str(&render_dividend(&dividend));
            }
        }

        out.push_str("_Caveat: ETF/LOF 可用 `fund_daily` 看交易所行情；开放式基金通常以 NAV 为主。基金工具提供事实上下文，不替代组合适配性、流动性和费率分析。_");
        Ok(out)
    }
}

#[derive(Default)]
struct Table {
    fields: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

impl Table {
    fn from_response(response: TushareResponse) -> Self {
        Self {
            fields: response.fields,
            rows: response.items,
        }
    }

    fn text(&self, row: &[serde_json::Value], field: &str) -> String {
        self.fields
            .iter()
            .position(|f| f == field)
            .and_then(|i| row.get(i))
            .map(value_text)
            .unwrap_or_default()
    }
}

fn fund_basic_params(fund_code: Option<&str>, market: &str) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    params.insert("market".to_string(), serde_json::json!(market));
    if let Some(code) = fund_code {
        params.insert("ts_code".to_string(), serde_json::json!(code));
    }
    serde_json::Value::Object(params)
}

fn infer_market(fund_code: Option<&str>) -> &'static str {
    match fund_code {
        Some(code) if code.ends_with(".OF") => "O",
        Some(code) if code.ends_with(".SH") || code.ends_with(".SZ") => "E",
        _ => "E",
    }
}

fn sections(args: &serde_json::Value, has_fund_code: bool) -> Vec<String> {
    args.get("sections")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            if has_fund_code {
                vec![
                    "profile".to_string(),
                    "quote".to_string(),
                    "nav".to_string(),
                    "portfolio".to_string(),
                    "share".to_string(),
                    "manager".to_string(),
                    "dividend".to_string(),
                ]
            } else {
                vec!["profile".to_string()]
            }
        })
}

fn limit(args: &serde_json::Value) -> usize {
    args.get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(1, MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
}

fn render_profile(table: &Table, limit: usize) -> String {
    let mut out = section_header("基金资料", "fund_basic", table.rows.len());
    if table.rows.is_empty() {
        out.push_str(
            "- 当前基金资料来源没有返回记录；请核验基金代码/市场，或把它作为数据缺口处理。\n\n",
        );
        return out;
    }
    for row in table.rows.iter().take(limit) {
        out.push_str(&format!(
            "- {} {} · {} · 管理人={} · 基准={} · 成立={} · 上市={} · 费率={}/{}\n",
            table.text(row, "ts_code"),
            table.text(row, "name"),
            table.text(row, "fund_type"),
            table.text(row, "management"),
            compact_text(&table.text(row, "benchmark"), 48),
            fmt_date(&table.text(row, "found_date")),
            fmt_date(&table.text(row, "list_date")),
            table.text(row, "m_fee"),
            table.text(row, "c_fee")
        ));
    }
    out.push('\n');
    out
}

fn render_quote(table: &Table) -> String {
    let mut out = section_header("交易所行情", "fund_daily", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No exchange quote returned. For open-end funds, prefer NAV.\n\n");
        return out;
    }
    for row in table.rows.iter().take(8) {
        out.push_str(&format!(
            "- {} · close={} · pct={}% · amount={}\n",
            fmt_date(&table.text(row, "trade_date")),
            table.text(row, "close"),
            table.text(row, "pct_chg"),
            table.text(row, "amount")
        ));
    }
    out.push('\n');
    out
}

fn render_nav(table: &Table) -> String {
    let mut out = section_header("NAV", "fund_nav", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No NAV rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(8) {
        out.push_str(&format!(
            "- {} · unit={} · accum={} · adj={}\n",
            fmt_date(&table.text(row, "ann_date")),
            table.text(row, "unit_nav"),
            table.text(row, "accum_nav"),
            table.text(row, "adj_nav")
        ));
    }
    out.push('\n');
    out
}

fn render_portfolio(table: &Table) -> String {
    let mut out = section_header("持仓", "fund_portfolio", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No portfolio rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(12) {
        out.push_str(&format!(
            "- {} · {} · 市值={} · 占净值={}% · 占流通={}%\n",
            fmt_date(&table.text(row, "end_date")),
            table.text(row, "symbol"),
            table.text(row, "mkv"),
            table.text(row, "stk_mkv_ratio"),
            table.text(row, "stk_float_ratio")
        ));
    }
    out.push('\n');
    out
}

fn render_share(table: &Table) -> String {
    let mut out = section_header("份额", "fund_share", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No share rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(8) {
        out.push_str(&format!(
            "- {} · fd_share={}\n",
            fmt_date(&table.text(row, "trade_date")),
            table.text(row, "fd_share")
        ));
    }
    out.push('\n');
    out
}

fn render_manager(table: &Table) -> String {
    let mut out = section_header("基金经理", "fund_manager", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No manager rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(5) {
        out.push_str(&format!(
            "- {} · {} → {} · {} · {}\n",
            table.text(row, "name"),
            fmt_date(&table.text(row, "begin_date")),
            fmt_date(&table.text(row, "end_date")),
            table.text(row, "edu"),
            compact_text(&table.text(row, "resume"), 120)
        ));
    }
    out.push('\n');
    out
}

fn render_dividend(table: &Table) -> String {
    let mut out = section_header("分红", "fund_div", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No dividend rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(8) {
        out.push_str(&format!(
            "- {} · {} · 每份={} · 登记={} · 支付={}\n",
            fmt_date(&table.text(row, "ann_date")),
            table.text(row, "div_proc"),
            table.text(row, "div_cash"),
            fmt_date(&table.text(row, "record_date")),
            fmt_date(&table.text(row, "pay_date"))
        ));
    }
    out.push('\n');
    out
}

fn section_header(title: &str, api: &str, rows: usize) -> String {
    format!("### {title} · {} · {rows}条\n", fund_source_label(api))
}

fn fund_source_label(api: &str) -> &'static str {
    match api {
        "fund_basic" => "基金资料",
        "fund_daily" => "交易所行情",
        "fund_nav" => "基金净值",
        "fund_portfolio" => "基金持仓",
        "fund_share" => "基金份额",
        "fund_manager" => "基金经理",
        "fund_div" => "基金分红",
        _ => "结构化基金来源",
    }
}

fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn fmt_date(s: &str) -> String {
    if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..])
    } else {
        s.to_string()
    }
}

fn compact_text(s: &str, max_chars: usize) -> String {
    let text = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_market_from_fund_code() {
        assert_eq!(infer_market(Some("510300.SH")), "E");
        assert_eq!(infer_market(Some("159915.SZ")), "E");
        assert_eq!(infer_market(Some("110022.OF")), "O");
        assert_eq!(infer_market(None), "E");
    }

    #[test]
    fn defaults_sections_by_code_presence() {
        assert_eq!(sections(&serde_json::json!({}), false), vec!["profile"]);
        let with_code = sections(&serde_json::json!({}), true);
        assert!(with_code.contains(&"profile".to_string()));
        assert!(with_code.contains(&"portfolio".to_string()));
        assert!(with_code.contains(&"dividend".to_string()));
    }

    #[test]
    fn renders_valid_empty_quote_as_data_gap() {
        let table = Table {
            fields: vec!["trade_date".into()],
            rows: Vec::new(),
        };
        let out = render_quote(&table);
        assert!(out.contains("No exchange quote returned"));
    }

    #[test]
    fn fund_basic_params_include_market_and_optional_code() {
        let params = fund_basic_params(Some("510300.SH"), "E");
        assert_eq!(params["market"], "E");
        assert_eq!(params["ts_code"], "510300.SH");
    }
}
