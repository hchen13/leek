use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{Datelike, Duration as ChronoDuration, FixedOffset, TimeZone, Utc, Weekday};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{ToolContext, ToolHandler, data_provider_tokens};

const TOOL_NAME: &str = "get_a_share_industry_context";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const EASTMONEY_UT: &str = "b2884a393a59ad64002292a3e90d46a5";
const EASTMONEY_BLOCK_FLOW_URL: &str = "https://emdatah5.eastmoney.com/dc/ZJLX/getZDYLBData";
const EASTMONEY_BLOCK_FLOW_FIELDS: &str = "f1,f2,f3,f4,f12,f13,f14,f62,f124,f128,f140,f141";

pub struct GetAShareIndustryContextTool {
    http: Client,
}

impl GetAShareIndustryContextTool {
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

    async fn fetch_tushare_industry_flow(
        &self,
        token: &str,
        classification: &Table,
        index_code: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<FlowEvidence>> {
        for trade_date in recent_trade_date_candidates(8) {
            let table = self
                .tushare_post(
                    token,
                    "moneyflow_ind_ths",
                    serde_json::json!({"trade_date": trade_date}),
                    "trade_date,ts_code,industry,lead_stock,close,pct_change,company_num,pct_change_stock,close_price,net_buy_amount,net_sell_amount,net_amount",
                    cancel,
                )
                .await?;
            if table.rows.is_empty() {
                continue;
            }
            let matches = matching_flow_rows(&table, classification, index_code);
            if let Some(row) = matches.first() {
                return Ok(Some(FlowEvidence {
                    source: "日频行业资金".to_string(),
                    date_or_time: fmt_date(&table.text(row, "trade_date")),
                    industry: table.text(row, "industry"),
                    code: table.text(row, "ts_code"),
                    price: table.text(row, "close"),
                    pct: table.text(row, "pct_change"),
                    main_net: table.text(row, "net_amount"),
                    lead_stock: table.text(row, "lead_stock"),
                    note: "日频；单位沿用结构化来源字段口径".to_string(),
                }));
            }
        }
        Ok(None)
    }

    async fn fetch_eastmoney_industry_flow(
        &self,
        names: &[String],
        cancel: &CancellationToken,
    ) -> Result<Option<FlowEvidence>> {
        for order in ["1", "0"] {
            let request = self
                .http
                .get(EASTMONEY_BLOCK_FLOW_URL)
                .query(&[
                    ("fields", EASTMONEY_BLOCK_FLOW_FIELDS.to_string()),
                    ("pn", "1".to_string()),
                    ("pz", "500".to_string()),
                    ("fid", "f62".to_string()),
                    ("po", order.to_string()),
                    ("fs", "m:90+t:2".to_string()),
                    ("ut", EASTMONEY_UT.to_string()),
                ])
                .build()?;
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(anyhow!("aborted before Eastmoney industry-flow request")),
                result = self.http.execute(request) => result?,
            };
            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("Eastmoney industry flow returned HTTP {status}"));
            }
            let text = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(anyhow!("aborted before Eastmoney industry-flow response body")),
                result = response.text() => result?,
            };
            let body: EastmoneyBlockFlowResponse = serde_json::from_str(&text).map_err(|err| {
                anyhow!(
                    "Eastmoney industry flow returned invalid JSON: {err}; sample: {}",
                    compact_text(&text, 160)
                )
            })?;
            if body.rc != Some(0) {
                return Err(anyhow!(
                    "Eastmoney industry flow returned rc={:?}: {}",
                    body.rc,
                    body.message_text().unwrap_or("unknown error")
                ));
            }
            let rows = body.data.map(|data| data.diff).unwrap_or_default();
            if let Some(row) = rows
                .iter()
                .find(|row| industry_name_matches(value_text_opt(row.f14.as_ref()).as_str(), names))
            {
                return Ok(Some(FlowEvidence {
                    source: "东方财富盘中板块资金".to_string(),
                    date_or_time: eastmoney_epoch_text(row.f124.as_ref()),
                    industry: value_text_opt(row.f14.as_ref()),
                    code: value_text_opt(row.f12.as_ref()),
                    price: value_text_opt(row.f2.as_ref()),
                    pct: value_text_opt(row.f3.as_ref()),
                    main_net: value_text_opt(row.f62.as_ref()),
                    lead_stock: format!(
                        "{}{}",
                        value_text_opt(row.f128.as_ref()),
                        suffix_code(row.f140.as_ref())
                    ),
                    note: "盘中/近实时；单位元；公开页面排行口径".to_string(),
                }));
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl ToolHandler for GetAShareIndustryContextTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Build A-share industry context for a stock or industry code. Use this to establish a session working model: Shenwan classification, selected industry index, recent industry price/volume behavior, peer sample, and sector capital-flow evidence. This is a context-building tool, not an investment conclusion. It separates valid empty results from source-unavailable gaps and keeps realtime public sector flow distinct from official daily industry flow."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ts_code": {
                        "type": "string",
                        "description": "A-share code such as 600519.SH. Required unless industry_code is provided."
                    },
                    "industry_code": {
                        "type": "string",
                        "description": "Shenwan index code when already known."
                    },
                    "level": {
                        "type": "string",
                        "enum": ["L1", "L2", "L3"],
                        "description": "Shenwan level, default L2 for stock lookup."
                    },
                    "include_members": {
                        "type": "boolean",
                        "description": "Whether to include a sample of peer constituents, default true."
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
        let ts_code = args
            .get("ts_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let industry_code = args
            .get("industry_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let include_members = args
            .get("include_members")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let level = args
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("L2")
            .trim()
            .to_uppercase();

        let classification = if let Some(code) = ts_code.as_deref() {
            self.tushare_post(
                &token,
                "index_member_all",
                serde_json::json!({"ts_code": code}),
                "l1_code,l1_name,l2_code,l2_name,l3_code,l3_name,ts_code,name,in_date,out_date,is_new",
                &cancel,
            )
            .await?
        } else {
            Table::empty()
        };

        let Some(selected_industry) =
            select_industry(&classification, industry_code.as_deref(), &level)
        else {
            return Ok("[get_a_share_industry_context: no Shenwan industry classification found. Treat this as a knowledge gap and research the business model from first principles.]".to_string());
        };
        let index_code = selected_industry.code.as_str();

        let index_daily = self
            .tushare_post(
                &token,
                "sw_daily",
                serde_json::json!({"ts_code": index_code, "limit": 8}),
                "ts_code,trade_date,name,open,high,low,close,change,pct_change,vol,amount",
                &cancel,
            )
            .await?;

        let members = if include_members {
            Some(
                self.tushare_post(
                    &token,
                    "index_member",
                    serde_json::json!({"index_code": index_code, "is_new": "Y"}),
                    "index_code,index_name,con_code,con_name,in_date,out_date,is_new",
                    &cancel,
                )
                .await?,
            )
        } else {
            None
        };

        let industry_names = industry_names(&classification, selected_industry.name.as_str());
        let eastmoney_flow = self
            .fetch_eastmoney_industry_flow(&industry_names, &cancel)
            .await
            .ok()
            .flatten();
        let tushare_flow = self
            .fetch_tushare_industry_flow(&token, &classification, index_code, &cancel)
            .await
            .ok()
            .flatten();
        let index_rows = index_rows(&index_daily);
        let index_summary = index_summary(&index_rows);

        let mut out = String::new();
        let subject = ts_code
            .as_deref()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| selected_industry.code.clone());
        out.push_str(&format!("## A股行业上下文 · {subject}\n\n"));
        out.push_str("### 任务摘要\n");
        if let Some(code) = ts_code.as_deref() {
            out.push_str(&format!("- 标的：{code}\n"));
        }
        out.push_str(&format!(
            "- 主行业：{} {}{}{}\n",
            selected_industry.level,
            selected_industry.name,
            if selected_industry.code.is_empty() {
                ""
            } else {
                " · "
            },
            selected_industry.code
        ));
        out.push_str(&format!(
            "- 行业指数近期表现：{}\n",
            index_summary
                .as_deref()
                .unwrap_or("行业指数日频来源未返回可计算序列")
        ));
        out.push_str(&format!(
            "- 资金面：{}\n",
            flow_summary(eastmoney_flow.as_ref(), tushare_flow.as_ref())
        ));

        out.push_str("\n### 申万分类\n");
        out.push_str("| 层级 | 代码 | 名称 |\n");
        out.push_str("|---|---|---|\n");
        if classification.rows.is_empty() {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                selected_industry.level, selected_industry.code, selected_industry.name
            ));
        } else {
            if let Some(row) = classification.rows.first() {
                for (level, code_field, name_field) in [
                    ("L1", "l1_code", "l1_name"),
                    ("L2", "l2_code", "l2_name"),
                    ("L3", "l3_code", "l3_name"),
                ] {
                    let code = classification.text(row, code_field);
                    let name = classification.text(row, name_field);
                    if !code.is_empty() || !name.is_empty() {
                        out.push_str(&format!("| {level} | {code} | {name} |\n"));
                    }
                }
            }
        }

        out.push_str("\n### 行业指数近期表现\n");
        out.push_str("| 日期 | 指数 | 收盘 | 涨跌幅% | 成交额 |\n");
        out.push_str("|---|---|---:|---:|---:|\n");
        if index_rows.is_empty() {
            out.push_str("| 无返回记录 | - | - | - | - |\n");
        } else {
            for row in index_rows.iter().rev().take(8).rev() {
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    row.date, row.name, row.close, row.pct, row.amount
                ));
            }
        }
        out.push_str(
            "_来源: 配置的行业指数日频来源；指数表现用于行业上下文，不等同于个股收益。_\n",
        );

        if let Some(members) = members {
            out.push_str("\n### 同业样本\n");
            out.push_str("| 代码 | 名称 |\n");
            out.push_str("|---|---|\n");
            if members.rows.is_empty() {
                out.push_str("| 无返回记录 | - |\n");
            } else {
                for row in members.rows.iter().take(12) {
                    out.push_str(&format!(
                        "| {} | {} |\n",
                        members.text(row, "con_code"),
                        members.text(row, "con_name")
                    ));
                }
            }
        }

        out.push_str("\n### 行业资金面\n");
        out.push_str("| 来源 | 时间/日期 | 行业 | 代码 | 点位/收盘 | 涨跌幅% | 主力净流入 | 领涨股 | 说明 |\n");
        out.push_str("|---|---|---|---|---:|---:|---:|---|---|\n");
        let mut flow_rows = 0usize;
        if let Some(flow) = eastmoney_flow.as_ref() {
            out.push_str(&flow.render_row());
            flow_rows += 1;
        }
        if let Some(flow) = tushare_flow.as_ref() {
            out.push_str(&flow.render_row());
            flow_rows += 1;
        }
        if flow_rows == 0 {
            out.push_str("| 来源不可用 | - | - | - | - | - | - | - | 行业资金流来源暂不可用；这不是资金流为零。继续使用行业指数、同业样本、成交额和后续结构化资金工具交叉验证。 |\n");
        }
        out.push_str("\n_Caveat: 行业分类和板块资金是 working model 的上下文，不是买卖信号。若 corpus 没有该产业链知识，应继续调研产业链、需求、渠道、政策和竞争格局。_");
        Ok(out)
    }
}

#[derive(Clone)]
struct IndustrySelection {
    code: String,
    level: String,
    name: String,
}

#[derive(Clone)]
struct IndexRow {
    date: String,
    name: String,
    close: String,
    high: Option<f64>,
    low: Option<f64>,
    pct: String,
    amount: String,
}

struct FlowEvidence {
    source: String,
    date_or_time: String,
    industry: String,
    code: String,
    price: String,
    pct: String,
    main_net: String,
    lead_stock: String,
    note: String,
}

impl FlowEvidence {
    fn render_row(&self) -> String {
        format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            self.source,
            self.date_or_time,
            self.industry,
            self.code,
            self.price,
            self.pct,
            self.main_net,
            self.lead_stock,
            self.note
        )
    }
}

#[derive(Deserialize)]
struct EastmoneyBlockFlowResponse {
    rc: Option<i64>,
    data: Option<EastmoneyBlockFlowData>,
    msg: Option<String>,
    message: Option<String>,
}

impl EastmoneyBlockFlowResponse {
    fn message_text(&self) -> Option<&str> {
        clean_opt(self.message.as_deref()).or_else(|| clean_opt(self.msg.as_deref()))
    }
}

#[derive(Deserialize)]
struct EastmoneyBlockFlowData {
    #[serde(default)]
    diff: Vec<EastmoneyBlockFlowRow>,
}

#[derive(Deserialize)]
struct EastmoneyBlockFlowRow {
    f2: Option<serde_json::Value>,
    f3: Option<serde_json::Value>,
    f12: Option<serde_json::Value>,
    f14: Option<serde_json::Value>,
    f62: Option<serde_json::Value>,
    f124: Option<serde_json::Value>,
    f128: Option<serde_json::Value>,
    f140: Option<serde_json::Value>,
}

#[derive(Default)]
struct Table {
    fields: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

impl Table {
    fn empty() -> Self {
        Self::default()
    }

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

fn select_industry(
    table: &Table,
    explicit_code: Option<&str>,
    requested_level: &str,
) -> Option<IndustrySelection> {
    let requested_level = normalize_level(requested_level);
    if let Some(code) = explicit_code.filter(|code| !code.trim().is_empty()) {
        if let Some(row) = table.rows.first() {
            for (level, code_field, name_field) in industry_level_fields(&requested_level) {
                if table.text(row, code_field) == code {
                    return Some(IndustrySelection {
                        code: code.to_string(),
                        level: level.to_string(),
                        name: table.text(row, name_field),
                    });
                }
            }
        }
        return Some(IndustrySelection {
            code: code.to_string(),
            level: requested_level,
            name: String::new(),
        });
    }

    let row = table.rows.first()?;
    industry_level_fields(&requested_level)
        .into_iter()
        .find_map(|(level, code_field, name_field)| {
            let code = table.text(row, code_field);
            let name = table.text(row, name_field);
            if code.is_empty() && name.is_empty() {
                None
            } else {
                Some(IndustrySelection {
                    code,
                    level: level.to_string(),
                    name,
                })
            }
        })
}

fn normalize_level(level: &str) -> String {
    match level.trim().to_uppercase().as_str() {
        "L1" => "L1".to_string(),
        "L3" => "L3".to_string(),
        _ => "L2".to_string(),
    }
}

fn industry_level_fields(requested_level: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    match requested_level {
        "L1" => vec![
            ("L1", "l1_code", "l1_name"),
            ("L2", "l2_code", "l2_name"),
            ("L3", "l3_code", "l3_name"),
        ],
        "L3" => vec![
            ("L3", "l3_code", "l3_name"),
            ("L2", "l2_code", "l2_name"),
            ("L1", "l1_code", "l1_name"),
        ],
        _ => vec![
            ("L2", "l2_code", "l2_name"),
            ("L1", "l1_code", "l1_name"),
            ("L3", "l3_code", "l3_name"),
        ],
    }
}

fn industry_names(table: &Table, fallback: &str) -> Vec<String> {
    let mut names = Vec::new();
    push_unique_name(&mut names, fallback);
    if let Some(row) = table.rows.first() {
        for field in ["l1_name", "l2_name", "l3_name"] {
            push_unique_name(&mut names, &table.text(row, field));
        }
    }
    names
}

fn push_unique_name(names: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    if !names.iter().any(|name| name == value) {
        names.push(value.to_string());
    }
}

fn index_rows(table: &Table) -> Vec<IndexRow> {
    let mut rows = table
        .rows
        .iter()
        .map(|row| IndexRow {
            date: fmt_date(&table.text(row, "trade_date")),
            name: table.text(row, "name"),
            close: table.text(row, "close"),
            high: parse_number(&table.text(row, "high")),
            low: parse_number(&table.text(row, "low")),
            pct: table.text(row, "pct_change"),
            amount: table.text(row, "amount"),
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.date.cmp(&b.date));
    rows
}

fn index_summary(rows: &[IndexRow]) -> Option<String> {
    let latest = rows.last()?;
    let first_close = rows.iter().find_map(|row| parse_number(&row.close));
    let latest_close = parse_number(&latest.close);
    let period_pct = first_close.zip(latest_close).and_then(|(first, last)| {
        (first.abs() > f64::EPSILON).then_some((last / first - 1.0) * 100.0)
    });
    let high = rows.iter().filter_map(|row| row.high).reduce(f64::max);
    let low = rows.iter().filter_map(|row| row.low).reduce(f64::min);
    let mut parts = vec![format!(
        "最新 {} close={} pct={}%",
        latest.date, latest.close, latest.pct
    )];
    if let Some(period_pct) = period_pct {
        parts.push(format!("{}日涨跌={period_pct:.2}%", rows.len()));
    }
    if let (Some(high), Some(low)) = (high, low) {
        parts.push(format!("区间高低={high:.2}/{low:.2}"));
    }
    if !latest.amount.is_empty() {
        parts.push(format!("最新成交额={}", latest.amount));
    }
    Some(parts.join("；"))
}

fn flow_summary(eastmoney: Option<&FlowEvidence>, tushare: Option<&FlowEvidence>) -> String {
    let mut parts = Vec::new();
    if let Some(flow) = eastmoney {
        parts.push(flow_summary_piece(flow));
    }
    if let Some(flow) = tushare {
        parts.push(flow_summary_piece(flow));
    }
    if parts.is_empty() {
        "资金流来源暂不可用；不要解释为零流入".to_string()
    } else {
        parts.join("；")
    }
}

fn flow_summary_piece(flow: &FlowEvidence) -> String {
    format!(
        "{} {} {} 主力净流入={} 涨跌幅={}%",
        flow.source, flow.date_or_time, flow.industry, flow.main_net, flow.pct
    )
}

fn matching_flow_rows<'a>(
    flow: &'a Table,
    classification: &Table,
    index_code: &str,
) -> Vec<&'a Vec<serde_json::Value>> {
    let names = industry_names(classification, "");
    flow.rows
        .iter()
        .filter(|row| {
            let code = flow.text(row, "ts_code");
            let industry = flow.text(row, "industry");
            code == index_code || industry_name_matches(&industry, &names)
        })
        .collect()
}

fn recent_trade_date_candidates(limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0i64;
    while out.len() < limit {
        let day = (Utc::now() - ChronoDuration::days(offset)).date_naive();
        offset += 1;
        if matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
            continue;
        }
        out.push(format!(
            "{:04}{:02}{:02}",
            day.year(),
            day.month(),
            day.day()
        ));
    }
    out
}

fn industry_name_matches(candidate: &str, names: &[String]) -> bool {
    let candidate = normalize_industry_name(candidate);
    if candidate.is_empty() {
        return false;
    }
    names
        .iter()
        .map(|name| normalize_industry_name(name))
        .filter(|name| !name.is_empty())
        .any(|name| candidate.contains(&name) || name.contains(&candidate))
}

fn normalize_industry_name(value: &str) -> String {
    value
        .trim()
        .replace('（', "(")
        .replace('）', ")")
        .replace(['Ⅰ', 'Ⅱ', 'Ⅲ'], "")
        .replace("III", "")
        .replace("II", "")
        .replace('I', "")
        .replace(' ', "")
}

fn parse_number(value: &str) -> Option<f64> {
    let cleaned = value.trim().replace([',', '，', '%'], "");
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn value_text_opt(v: Option<&serde_json::Value>) -> String {
    v.map(value_text)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "-")
        .unwrap_or_default()
}

fn value_i64(v: Option<&serde_json::Value>) -> Option<i64> {
    match v? {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|v| v as i64)),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn eastmoney_epoch_text(v: Option<&serde_json::Value>) -> String {
    let Some(raw) = value_i64(v) else {
        return String::new();
    };
    let seconds = if raw > 9_999_999_999 { raw / 1000 } else { raw };
    let Some(utc) = Utc.timestamp_opt(seconds, 0).single() else {
        return raw.to_string();
    };
    let Some(offset) = FixedOffset::east_opt(8 * 3600) else {
        return utc.to_rfc3339();
    };
    utc.with_timezone(&offset)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn suffix_code(v: Option<&serde_json::Value>) -> String {
    let code = value_text_opt(v);
    if code.is_empty() {
        String::new()
    } else {
        format!(" ({code})")
    }
}

fn compact_text(text: &str, max_chars: usize) -> String {
    let text = text.trim().replace(['\n', '\r', '\t'], " ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push('…');
    out
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

fn clean_opt(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_l2_before_other_levels() {
        let table = Table {
            fields: vec![
                "l1_code".into(),
                "l1_name".into(),
                "l2_code".into(),
                "l2_name".into(),
                "l3_code".into(),
                "l3_name".into(),
            ],
            rows: vec![vec![
                serde_json::json!("801120.SI"),
                serde_json::json!("食品饮料"),
                serde_json::json!("801123.SI"),
                serde_json::json!("白酒"),
                serde_json::json!("80112301.SI"),
                serde_json::json!("白酒III"),
            ]],
        };
        let selected = select_industry(&table, None, "L2").unwrap();
        assert_eq!(selected.code, "801123.SI");
        assert_eq!(selected.name, "白酒");
    }

    #[test]
    fn matching_flow_uses_industry_names() {
        let classification = Table {
            fields: vec!["l1_name".into(), "l2_name".into()],
            rows: vec![vec![
                serde_json::json!("食品饮料"),
                serde_json::json!("白酒"),
            ]],
        };
        let flow = Table {
            fields: vec!["ts_code".into(), "industry".into()],
            rows: vec![
                vec![serde_json::json!("x"), serde_json::json!("白酒")],
                vec![serde_json::json!("y"), serde_json::json!("银行")],
            ],
        };
        assert_eq!(matching_flow_rows(&flow, &classification, "z").len(), 1);
    }

    #[test]
    fn industry_name_matching_ignores_level_suffixes() {
        let names = vec!["白酒III".to_string()];
        assert!(industry_name_matches("白酒", &names));
        let names = vec!["白酒".to_string()];
        assert!(industry_name_matches("白酒Ⅱ", &names));
    }

    #[test]
    fn index_summary_compacts_recent_sequence() {
        let rows = vec![
            IndexRow {
                date: "2026-05-20".to_string(),
                name: "白酒".to_string(),
                close: "100".to_string(),
                high: Some(102.0),
                low: Some(98.0),
                pct: "1.0".to_string(),
                amount: "1000".to_string(),
            },
            IndexRow {
                date: "2026-05-21".to_string(),
                name: "白酒".to_string(),
                close: "103".to_string(),
                high: Some(104.0),
                low: Some(99.0),
                pct: "3.0".to_string(),
                amount: "1200".to_string(),
            },
        ];
        let summary = index_summary(&rows).unwrap();
        assert!(summary.contains("2日涨跌=3.00%"));
        assert!(summary.contains("区间高低=104.00/98.00"));
    }
}
