use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{Datelike, Utc};
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{ToolContext, ToolHandler, data_provider_tokens};

const TOOL_NAME: &str = "get_a_share_research_sources";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_DAYS: i64 = 180;
const MAX_LIMIT: usize = 20;
const EASTMONEY_REPORT_LIST_URL: &str = "https://reportapi.eastmoney.com/report/list";
const EASTMONEY_BOARD_LIST_URL: &str = "https://datacenter-web.eastmoney.com/api/data/v1/get";

pub struct GetAShareResearchSourcesTool {
    http: Client,
}

impl GetAShareResearchSourcesTool {
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
    ) -> Result<TushareTable> {
        let client = TushareClient::with_client(token, self.http.clone())?;
        let response = client
            .query_cancelled(api_name, params, fields, cancel)
            .await?;
        Ok(TushareTable::from_response(response))
    }

    async fn fetch_announcements(
        &self,
        token: &str,
        ts_code: Option<&str>,
        start_date: &str,
        end_date: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Section> {
        let mut params = serde_json::Map::new();
        params.insert("start_date".to_string(), serde_json::json!(start_date));
        params.insert("end_date".to_string(), serde_json::json!(end_date));
        if let Some(code) = ts_code {
            params.insert("ts_code".to_string(), serde_json::json!(code));
        }
        let table = self
            .tushare_post(
                token,
                "anns_d",
                serde_json::Value::Object(params),
                "ts_code,name,title,ann_date,url,rec_time",
                cancel,
            )
            .await?;
        let mut lines = Vec::new();
        for row in table.rows.iter().take(limit) {
            let title = table.text(row, "title");
            if title.is_empty() {
                continue;
            }
            let date = table.text(row, "ann_date");
            let code = table.text(row, "ts_code");
            let url = table.text(row, "url");
            lines.push(format!(
                "- {} · {} · {}{}",
                fmt_date(&date),
                code,
                title,
                suffix_url(&url)
            ));
        }
        Ok(Section::new("公告", "anns_d", lines, table.rows.len()))
    }

    async fn fetch_eastmoney_reports(
        &self,
        ts_code: Option<&str>,
        industry_code: Option<&str>,
        industry_name: Option<&str>,
        start_date: &str,
        end_date: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Section> {
        let selected_industry_code = if let Some(code) = industry_code {
            Some(code.to_string())
        } else if let Some(name) = industry_name {
            Some(self.resolve_eastmoney_industry_code(name, cancel).await?)
        } else {
            None
        };
        let report_scope = if selected_industry_code.is_some() {
            EastmoneyReportScope::Industry
        } else {
            EastmoneyReportScope::Stock
        };
        let mut params = vec![
            (
                "industryCode",
                selected_industry_code.as_deref().unwrap_or("*").to_string(),
            ),
            ("pageSize", limit.to_string()),
            ("industry", "*".to_string()),
            ("rating", "*".to_string()),
            ("ratingChange", "*".to_string()),
            ("beginTime", eastmoney_date(start_date)),
            ("endTime", eastmoney_date(end_date)),
            ("pageNo", "1".to_string()),
            ("fields", String::new()),
            (
                "qType",
                if report_scope == EastmoneyReportScope::Industry {
                    "1"
                } else {
                    "0"
                }
                .to_string(),
            ),
            ("orgCode", String::new()),
            ("rcode", String::new()),
        ];
        if let Some(code) = ts_code {
            if report_scope == EastmoneyReportScope::Stock {
                params.push(("code", eastmoney_stock_code(code)));
            }
        } else if report_scope == EastmoneyReportScope::Stock {
            params.push(("code", "*".to_string()));
        }

        let request = self
            .http
            .get(EASTMONEY_REPORT_LIST_URL)
            .query(&params)
            .build()?;
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("aborted before Eastmoney report request")),
            result = self.http.execute(request) => result?,
        };
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("Eastmoney report list returned HTTP {status}"));
        }
        let text = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("aborted before Eastmoney report response body")),
            result = response.text() => result?,
        };
        let mut body: EastmoneyReportList = serde_json::from_str(&text).map_err(|err| {
            anyhow!(
                "Eastmoney report list returned invalid JSON: {err}; sample: {}",
                compact_text(&text, 160)
            )
        })?;
        if body.data.is_none() && body.hits.is_none() {
            if let Some(message) = body.message_text() {
                return Err(anyhow!("Eastmoney report list returned no data: {message}"));
            }
        }

        let rows = body.data.take().unwrap_or_default();
        let total_rows = body.hits.unwrap_or(rows.len());
        let mut lines = Vec::new();
        for row in rows.iter().take(limit) {
            if let Some(line) = eastmoney_report_line(row, report_scope) {
                lines.push(line);
            }
        }
        Ok(Section::new(
            if report_scope == EastmoneyReportScope::Industry {
                "研报（东方财富行业）"
            } else {
                "研报（东方财富）"
            },
            "eastmoney_report_list",
            lines,
            total_rows,
        ))
    }

    async fn resolve_eastmoney_industry_code(
        &self,
        industry_name: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let params = vec![
            ("reportName", "RPT_EMBOARD_ALL"),
            (
                "columns",
                "BOARD_CODE,BOARD_NAME,BOARD_CODE_BK,BOARD_LEVEL,BOARD_TYPE,BOARD_TYPE_NAME,FIRST_LETTER",
            ),
            ("quoteColumns", ""),
            ("sortColumns", "FIRST_LETTER"),
            ("sortTypes", "1"),
            ("source", "WEB"),
            ("client", "WEB"),
            ("filter", "(BOARD_TYPE=2)"),
            ("pageNumber", "1"),
            ("pageSize", "500"),
        ];
        let request = self
            .http
            .get(EASTMONEY_BOARD_LIST_URL)
            .query(&params)
            .build()?;
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("aborted before Eastmoney industry lookup request")),
            result = self.http.execute(request) => result?,
        };
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("Eastmoney industry lookup returned HTTP {status}"));
        }
        let text = tokio::select! {
            _ = cancel.cancelled() => return Err(anyhow!("aborted before Eastmoney industry lookup response body")),
            result = response.text() => result?,
        };
        let body: EastmoneyBoardList = serde_json::from_str(&text).map_err(|err| {
            anyhow!(
                "Eastmoney industry lookup returned invalid JSON: {err}; sample: {}",
                compact_text(&text, 160)
            )
        })?;
        let message = body.message_text().map(ToOwned::to_owned);
        let rows = body.result.map(|result| result.data).unwrap_or_default();
        select_eastmoney_industry_code(&rows, industry_name).ok_or_else(|| {
            let message = message.as_deref().unwrap_or("no matching industry code");
            anyhow!("Eastmoney industry lookup could not resolve {industry_name}: {message}")
        })
    }

    async fn fetch_tushare_reports(
        &self,
        token: &str,
        ts_code: Option<&str>,
        start_date: &str,
        end_date: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Section> {
        let mut params = serde_json::Map::new();
        params.insert("start_date".to_string(), serde_json::json!(start_date));
        params.insert("end_date".to_string(), serde_json::json!(end_date));
        if let Some(code) = ts_code {
            params.insert("ts_code".to_string(), serde_json::json!(code));
        }
        let table = self
            .tushare_post(
                token,
                "research_report",
                serde_json::Value::Object(params),
                "ts_code,name,title,report_date,org_name,author,abstr,url",
                cancel,
            )
            .await?;
        let mut lines = Vec::new();
        for row in table.rows.iter().take(limit) {
            let title = table.text(row, "title");
            if title.is_empty() {
                continue;
            }
            let date = table.text(row, "report_date");
            let org = table.text(row, "org_name");
            let abstr = compact_text(&table.text(row, "abstr"), 120);
            lines.push(format!(
                "- {} · {} · {}{}{}",
                fmt_date(&date),
                org,
                title,
                if abstr.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", abstr)
                },
                suffix_url(&table.text(row, "url"))
            ));
        }
        Ok(Section::new(
            "研报（Tushare fallback）",
            "research_report:fallback",
            lines,
            table.rows.len(),
        ))
    }

    async fn fetch_ir_qa(
        &self,
        token: &str,
        ts_code: &str,
        start_date: &str,
        end_date: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Section> {
        let (api_name, fields) = if ts_code.ends_with(".SH") {
            (
                "irm_qa_sh",
                "ts_code,name,trade_date,q,a,pub_time,industry,main_business",
            )
        } else if ts_code.ends_with(".SZ") {
            ("irm_qa_sz", "ts_code,name,trade_date,q,a,pub_time")
        } else {
            return Ok(Section::new(
                "互动问答",
                "irm_qa",
                vec!["- 北京证券交易所互动问答暂未接入。".to_string()],
                0,
            ));
        };
        let table = self
            .tushare_post(
                token,
                api_name,
                serde_json::json!({
                    "ts_code": ts_code,
                    "start_date": start_date,
                    "end_date": end_date,
                }),
                fields,
                cancel,
            )
            .await?;
        let mut lines = Vec::new();
        for row in table.rows.iter().take(limit) {
            let q = compact_text(&table.text(row, "q"), 80);
            let a = compact_text(&table.text(row, "a"), 120);
            if q.is_empty() && a.is_empty() {
                continue;
            }
            let date = table.text(row, "trade_date");
            lines.push(format!("- {} · Q: {} · A: {}", fmt_date(&date), q, a));
        }
        Ok(Section::new("互动问答", api_name, lines, table.rows.len()))
    }

    async fn fetch_business_events(
        &self,
        token: &str,
        ts_code: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Vec<Section>> {
        let mut sections = Vec::new();

        let mainbz = self
            .tushare_post(
                token,
                "fina_mainbz_vip",
                serde_json::json!({"ts_code": ts_code, "type": "P", "limit": limit}),
                "ts_code,end_date,bz_item,bz_sales,bz_profit,bz_cost,curr_type",
                cancel,
            )
            .await?;
        let mut main_lines = Vec::new();
        for row in mainbz.rows.iter().take(limit) {
            let item = mainbz.text(row, "bz_item");
            if item.is_empty() {
                continue;
            }
            main_lines.push(format!(
                "- {} · {} · 收入{} · 毛利{}",
                fmt_date(&mainbz.text(row, "end_date")),
                item,
                fmt_num(&mainbz.text(row, "bz_sales")),
                fmt_num(&mainbz.text(row, "bz_profit"))
            ));
        }
        sections.push(Section::new(
            "主营构成",
            "fina_mainbz_vip",
            main_lines,
            mainbz.rows.len(),
        ));

        let forecast = self
            .tushare_post(
                token,
                "forecast_vip",
                serde_json::json!({"ts_code": ts_code, "limit": limit}),
                "ts_code,ann_date,end_date,type,p_change_min,p_change_max,net_profit_min,net_profit_max,summary,change_reason",
                cancel,
            )
            .await?;
        let mut forecast_lines = Vec::new();
        for row in forecast.rows.iter().take(limit) {
            forecast_lines.push(format!(
                "- {} · {} · {} · {}",
                fmt_date(&forecast.text(row, "ann_date")),
                fmt_date(&forecast.text(row, "end_date")),
                forecast.text(row, "type"),
                compact_text(&forecast.text(row, "summary"), 110)
            ));
        }
        sections.push(Section::new(
            "业绩预告",
            "forecast_vip",
            forecast_lines,
            forecast.rows.len(),
        ));

        let dividend = self
            .tushare_post(
                token,
                "dividend",
                serde_json::json!({"ts_code": ts_code, "limit": limit}),
                "ts_code,end_date,ann_date,div_proc,stk_div,cash_div_tax,record_date,ex_date,pay_date",
                cancel,
            )
            .await?;
        let mut dividend_lines = Vec::new();
        for row in dividend.rows.iter().take(limit) {
            dividend_lines.push(format!(
                "- {} · {} · 派现{} · 股权登记{}",
                fmt_date(&dividend.text(row, "ann_date")),
                dividend.text(row, "div_proc"),
                dividend.text(row, "cash_div_tax"),
                fmt_date(&dividend.text(row, "record_date"))
            ));
        }
        sections.push(Section::new(
            "分红",
            "dividend",
            dividend_lines,
            dividend.rows.len(),
        ));

        Ok(sections)
    }
}

#[async_trait]
impl ToolHandler for GetAShareResearchSourcesTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Find structured A-share research source material: public broker research reports first, with a configured report-data fallback; announcements, exchange IR Q&A, business breakdowns, forecasts, and dividends from configured structured sources. Use this before relying on open web search when the task needs auditable A-share source evidence. This does not fetch generic news or make broker reports primary facts. Empty results are valid; permission/rate-limit errors mean the source is inaccessible, not that the event did not happen."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ts_code": {
                        "type": "string",
                        "description": "Optional A-share code such as 600519.SH. Required for ir_qa and business_events; research_reports converts it to the Eastmoney stock code when using the primary source."
                    },
                    "industry_code": {
                        "type": "string",
                        "description": "Optional public report-center industry code such as 1033. When set, research_reports returns industry reports instead of stock reports."
                    },
                    "industry_name": {
                        "type": "string",
                        "description": "Optional industry name such as 电池 or 白酒. Used to resolve a public report-center industry code when industry_code is not provided."
                    },
                    "source_types": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["announcements", "research_reports", "ir_qa", "business_events"]
                        },
                        "description": "Default: announcements, research_reports, business_events when ts_code is present; without ts_code only announcements and research_reports. research_reports uses the public report center first and falls back to a configured structured source only if the primary source is inaccessible. Add ir_qa only when exchange IR Q&A or management responses are specifically useful."
                    },
                    "start_date": {
                        "type": "string",
                        "description": "YYYYMMDD. Default: 180 calendar days before today."
                    },
                    "end_date": {
                        "type": "string",
                        "description": "YYYYMMDD. Default: today."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Rows per section, default 5, max 20."
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
        let mut tushare_token = None;
        let ts_code = args
            .get("ts_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let industry_code = args
            .get("industry_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let industry_name = args
            .get("industry_name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let (start_date, end_date) = date_window(&args);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_LIMIT))
            .unwrap_or(5);
        let source_types = source_types(&args, ts_code.is_some());

        let mut sections = Vec::new();
        if source_types.iter().any(|s| s == "announcements") {
            let result = match ensure_tushare_token(&mut tushare_token, ctx).await {
                Ok(token) => {
                    self.fetch_announcements(
                        token,
                        ts_code.as_deref(),
                        &start_date,
                        &end_date,
                        limit,
                        &cancel,
                    )
                    .await
                }
                Err(err) => Err(err),
            };
            push_section_result(&mut sections, "公告", "anns_d", result, &cancel)?;
        }
        if source_types.iter().any(|s| s == "research_reports") {
            let result = match self
                .fetch_eastmoney_reports(
                    ts_code.as_deref(),
                    industry_code.as_deref(),
                    industry_name.as_deref(),
                    &start_date,
                    &end_date,
                    limit,
                    &cancel,
                )
                .await
            {
                Ok(section) => Ok(section),
                Err(err) if is_abort_error(&err, &cancel) => Err(err),
                Err(eastmoney_err) => match ensure_tushare_token(&mut tushare_token, ctx).await {
                    Ok(token) => self
                        .fetch_tushare_reports(
                            token,
                            ts_code.as_deref(),
                            &start_date,
                            &end_date,
                            limit,
                            &cancel,
                        )
                        .await
                        .map_err(|tushare_err| {
                            anyhow!(
                                "Eastmoney report source unavailable: {}; Tushare fallback also failed: {}",
                                compact_text(&eastmoney_err.to_string(), 160),
                                compact_text(&tushare_err.to_string(), 160)
                            )
                        }),
                    Err(token_err) => Err(anyhow!(
                        "Eastmoney report source unavailable: {}; fallback report source unavailable: {}",
                        compact_text(&eastmoney_err.to_string(), 160),
                        token_err
                    )),
                },
            };
            push_section_result(
                &mut sections,
                "研报",
                "eastmoney_report_list",
                result,
                &cancel,
            )?;
        }
        if source_types.iter().any(|s| s == "ir_qa") {
            let code = ts_code
                .as_deref()
                .ok_or_else(|| anyhow!("ts_code is required for ir_qa"))?;
            let result = match ensure_tushare_token(&mut tushare_token, ctx).await {
                Ok(token) => {
                    self.fetch_ir_qa(token, code, &start_date, &end_date, limit, &cancel)
                        .await
                }
                Err(err) => Err(err),
            };
            push_section_result(
                &mut sections,
                "互动问答",
                if code.ends_with(".SH") {
                    "irm_qa_sh"
                } else if code.ends_with(".SZ") {
                    "irm_qa_sz"
                } else {
                    "irm_qa"
                },
                result,
                &cancel,
            )?;
        }
        if source_types.iter().any(|s| s == "business_events") {
            let code = ts_code
                .as_deref()
                .ok_or_else(|| anyhow!("ts_code is required for business_events"))?;
            let result = match ensure_tushare_token(&mut tushare_token, ctx).await {
                Ok(token) => {
                    self.fetch_business_events(token, code, limit, &cancel)
                        .await
                }
                Err(err) => Err(err),
            };
            match result {
                Ok(items) => sections.extend(items),
                Err(err) if is_abort_error(&err, &cancel) => return Err(err),
                Err(err) => {
                    sections.push(Section::unavailable("经营事件", "business_events", &err))
                }
            }
        }

        let subject = ts_code
            .as_deref()
            .map(ToOwned::to_owned)
            .or_else(|| industry_name.as_deref().map(|name| format!("行业 {name}")))
            .or_else(|| {
                industry_code
                    .as_deref()
                    .map(|code| format!("行业代码 {code}"))
            })
            .unwrap_or_else(|| "A-share market".to_string());
        let mut out = format!(
            "## {} · A股研究源材料\n\n窗口：{} → {}\n\n",
            subject,
            fmt_date(&start_date),
            fmt_date(&end_date)
        );
        for section in sections {
            out.push_str(&section.render());
            out.push('\n');
        }
        out.push_str(
            "_Sources: Eastmoney public report center for research_reports; Tushare Pro for announcements, IR Q&A, business breakdowns, forecasts, and dividends. News feeds are intentionally not included._",
        );
        Ok(out)
    }
}

async fn ensure_tushare_token<'a>(
    token: &'a mut Option<String>,
    ctx: &ToolContext,
) -> Result<&'a str> {
    if token.is_none() {
        *token = Some(data_provider_tokens::tushare_token(ctx).await?);
    }
    Ok(token.as_deref().expect("token initialized"))
}

fn push_section_result(
    sections: &mut Vec<Section>,
    title: &'static str,
    api: &'static str,
    result: Result<Section>,
    cancel: &CancellationToken,
) -> Result<()> {
    match result {
        Ok(section) => sections.push(section),
        Err(err) if is_abort_error(&err, cancel) => return Err(err),
        Err(err) => sections.push(Section::unavailable(title, api, &err)),
    }
    Ok(())
}

fn is_abort_error(err: &anyhow::Error, cancel: &CancellationToken) -> bool {
    cancel.is_cancelled() || err.to_string().to_lowercase().contains("aborted")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EastmoneyReportScope {
    Stock,
    Industry,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EastmoneyReportList {
    hits: Option<usize>,
    data: Option<Vec<EastmoneyReportRow>>,
    message: Option<String>,
    msg: Option<String>,
}

impl EastmoneyReportList {
    fn message_text(&self) -> Option<&str> {
        clean_opt(self.message.as_deref()).or_else(|| clean_opt(self.msg.as_deref()))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EastmoneyReportRow {
    title: Option<String>,
    stock_name: Option<String>,
    stock_code: Option<String>,
    org_name: Option<String>,
    org_s_name: Option<String>,
    publish_date: Option<String>,
    info_code: Option<String>,
    researcher: Option<String>,
    em_rating_name: Option<String>,
    s_rating_name: Option<String>,
    industry_name: Option<String>,
    indv_indu_name: Option<String>,
    attach_pages: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EastmoneyBoardList {
    result: Option<EastmoneyBoardResult>,
    message: Option<String>,
    msg: Option<String>,
}

impl EastmoneyBoardList {
    fn message_text(&self) -> Option<&str> {
        clean_opt(self.message.as_deref()).or_else(|| clean_opt(self.msg.as_deref()))
    }
}

#[derive(Deserialize)]
struct EastmoneyBoardResult {
    #[serde(default)]
    data: Vec<EastmoneyBoardRow>,
}

#[derive(Deserialize)]
struct EastmoneyBoardRow {
    #[serde(rename = "BOARD_CODE")]
    board_code: Option<String>,
    #[serde(rename = "BOARD_NAME")]
    board_name: Option<String>,
    #[serde(rename = "BOARD_LEVEL")]
    board_level: Option<String>,
}

fn select_eastmoney_industry_code(
    rows: &[EastmoneyBoardRow],
    industry_name: &str,
) -> Option<String> {
    let query = industry_name.trim();
    if query.is_empty() {
        return None;
    }

    rows.iter()
        .find(|row| clean_opt(row.board_name.as_deref()) == Some(query))
        .and_then(eastmoney_board_code)
        .or_else(|| {
            rows.iter()
                .find(|row| {
                    clean_opt(row.board_name.as_deref()).is_some_and(|name| name.contains(query))
                        && clean_opt(row.board_level.as_deref()) == Some("2")
                })
                .and_then(eastmoney_board_code)
        })
        .or_else(|| {
            rows.iter()
                .find(|row| {
                    clean_opt(row.board_name.as_deref()).is_some_and(|name| name.contains(query))
                })
                .and_then(eastmoney_board_code)
        })
}

fn eastmoney_board_code(row: &EastmoneyBoardRow) -> Option<String> {
    clean_opt(row.board_code.as_deref()).map(ToOwned::to_owned)
}

fn eastmoney_report_line(row: &EastmoneyReportRow, scope: EastmoneyReportScope) -> Option<String> {
    let title = clean_opt(row.title.as_deref())?;
    let date = clean_opt(row.publish_date.as_deref())
        .map(eastmoney_publish_date)
        .unwrap_or_else(|| "-".to_string());
    let org = clean_opt(row.org_s_name.as_deref())
        .or_else(|| clean_opt(row.org_name.as_deref()))
        .unwrap_or("未知机构");
    let mut parts = vec![format!("- {date}"), org.to_string(), title.to_string()];

    if let Some(target) = eastmoney_report_target(row, scope) {
        parts.push(target);
    }
    if let Some(rating) =
        clean_opt(row.em_rating_name.as_deref()).or_else(|| clean_opt(row.s_rating_name.as_deref()))
    {
        parts.push(format!("评级:{rating}"));
    }
    if let Some(researcher) = clean_opt(row.researcher.as_deref()) {
        parts.push(format!("作者:{researcher}"));
    }
    if let Some(pages) = row
        .attach_pages
        .as_ref()
        .map(value_text)
        .and_then(|v| clean_owned(v))
    {
        parts.push(format!("{pages}页"));
    }
    if let Some(info_code) = clean_opt(row.info_code.as_deref()) {
        parts.push(format!("详情页 {}", eastmoney_detail_url(info_code, scope)));
        parts.push(format!("PDF {}", eastmoney_pdf_url(info_code)));
    }

    Some(parts.join(" · "))
}

fn eastmoney_report_target(
    row: &EastmoneyReportRow,
    scope: EastmoneyReportScope,
) -> Option<String> {
    if scope == EastmoneyReportScope::Industry {
        return clean_opt(row.indv_indu_name.as_deref())
            .or_else(|| clean_opt(row.industry_name.as_deref()))
            .map(|name| format!("行业:{name}"));
    }

    let name = clean_opt(row.stock_name.as_deref());
    let code = clean_opt(row.stock_code.as_deref());
    match (name, code) {
        (Some(name), Some(code)) => Some(format!("标的:{name}({code})")),
        (Some(name), None) => Some(format!("标的:{name}")),
        (None, Some(code)) => Some(format!("标的:{code}")),
        (None, None) => clean_opt(row.indv_indu_name.as_deref()).map(|name| format!("行业:{name}")),
    }
}

fn eastmoney_detail_url(info_code: &str, scope: EastmoneyReportScope) -> String {
    if scope == EastmoneyReportScope::Industry {
        format!("https://data.eastmoney.com/report/zw_industry.jshtml?infocode={info_code}")
    } else {
        format!("https://data.eastmoney.com/report/info/{info_code}.html")
    }
}

fn eastmoney_pdf_url(info_code: &str) -> String {
    format!("https://pdf.dfcfw.com/pdf/H3_{info_code}_1.pdf")
}

struct TushareTable {
    fields: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

impl TushareTable {
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

struct Section {
    title: &'static str,
    api: &'static str,
    lines: Vec<String>,
    total_rows: usize,
    unavailable: Option<String>,
}

impl Section {
    fn new(title: &'static str, api: &'static str, lines: Vec<String>, total_rows: usize) -> Self {
        Self {
            title,
            api,
            lines,
            total_rows,
            unavailable: None,
        }
    }

    fn unavailable(title: &'static str, api: &'static str, err: &anyhow::Error) -> Self {
        Self {
            title,
            api,
            lines: Vec::new(),
            total_rows: 0,
            unavailable: Some(compact_text(&err.to_string(), 180)),
        }
    }

    fn render(self) -> String {
        let source = research_source_label(self.api);
        let mut out = if self.unavailable.is_some() {
            format!("### {} · `{source}` · 来源不可用\n", self.title)
        } else {
            format!("### {} · `{source}` · {}条\n", self.title, self.total_rows)
        };
        if let Some(reason) = self.unavailable {
            out.push_str(&format!(
                "- 来源不可用：{reason}。这代表覆盖/访问缺口，不是有效空结果。\n"
            ));
            return out;
        }
        if self.lines.is_empty() {
            out.push_str(
                "- 当前来源没有返回记录。这是有效空结果，不代表来源覆盖之外没有相关事实。\n",
            );
        } else {
            for line in self.lines {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }
}

fn research_source_label(api: &str) -> &'static str {
    match api {
        "anns_d" => "交易所公告",
        "eastmoney_report_center" => "东方财富研报中心",
        "research_report" | "research_report:fallback" => "结构化研报",
        "irm_qa_sh" | "irm_qa_sz" => "交易所互动问答",
        "business_events" => "经营事件",
        "source" => "原文来源",
        _ => "结构化来源",
    }
}

fn source_types(args: &serde_json::Value, has_ts_code: bool) -> Vec<String> {
    args.get("source_types")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            if has_ts_code {
                vec![
                    "announcements".to_string(),
                    "research_reports".to_string(),
                    "business_events".to_string(),
                ]
            } else {
                vec!["announcements".to_string(), "research_reports".to_string()]
            }
        })
}

fn date_window(args: &serde_json::Value) -> (String, String) {
    let today = Utc::now().date_naive();
    let start = today - chrono::Duration::days(DEFAULT_DAYS);
    let start_date = args
        .get("start_date")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| yyyymmdd(start));
    let end_date = args
        .get("end_date")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| yyyymmdd(today));
    (start_date, end_date)
}

fn yyyymmdd(d: chrono::NaiveDate) -> String {
    format!("{:04}{:02}{:02}", d.year(), d.month(), d.day())
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

fn eastmoney_date(s: &str) -> String {
    fmt_date(s)
}

fn eastmoney_publish_date(s: &str) -> String {
    if s.len() >= 10 && s.as_bytes().get(4) == Some(&b'-') && s.as_bytes().get(7) == Some(&b'-') {
        s[..10].to_string()
    } else {
        fmt_date(s)
    }
}

fn eastmoney_stock_code(ts_code: &str) -> String {
    ts_code
        .split('.')
        .next()
        .unwrap_or(ts_code)
        .trim()
        .to_string()
}

fn clean_opt(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

fn clean_owned(s: String) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn compact_text(s: &str, max_chars: usize) -> String {
    let mut text = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect::<String>();
        text.push('…');
    }
    text
}

fn suffix_url(url: &str) -> String {
    if url.trim().is_empty() {
        String::new()
    } else {
        format!(" · {}", url.trim())
    }
}

fn fmt_num(s: &str) -> String {
    s.parse::<f64>()
        .map(|v| format!("{:.2}", v / 100_000_000.0))
        .unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_source_types_by_ts_code_presence() {
        let args = serde_json::json!({});
        assert_eq!(
            source_types(&args, false),
            vec!["announcements".to_string(), "research_reports".to_string()]
        );
        assert_eq!(
            source_types(&args, true),
            vec![
                "announcements".to_string(),
                "research_reports".to_string(),
                "business_events".to_string()
            ]
        );
    }

    #[test]
    fn renders_empty_section_as_valid_empty() {
        let out = Section::new("公告", "anns_d", Vec::new(), 0).render();
        assert!(out.contains("有效空结果"));
        assert!(out.contains("交易所公告"));
        assert!(!out.contains("Tushare coverage"));
    }

    #[test]
    fn renders_unavailable_section_as_access_gap() {
        let out = Section::unavailable("互动问答", "irm_qa_sh", &anyhow::anyhow!("rate limited"))
            .render();
        assert!(out.contains("来源不可用"));
        assert!(out.contains("覆盖/访问缺口"));
        assert!(!out.contains("No rows returned"));
    }

    #[test]
    fn compact_text_truncates_on_character_count() {
        let text = compact_text("abcdef", 3);
        assert_eq!(text, "abc…");
    }

    #[test]
    fn eastmoney_helpers_normalize_inputs() {
        assert_eq!(eastmoney_stock_code("600519.SH"), "600519");
        assert_eq!(eastmoney_date("20260524"), "2026-05-24");
        assert_eq!(
            eastmoney_publish_date("2026-05-05 00:00:00.000"),
            "2026-05-05"
        );
    }

    #[test]
    fn eastmoney_report_line_exposes_source_links() {
        let row = EastmoneyReportRow {
            title: Some("公司事件点评报告".to_string()),
            stock_name: Some("贵州茅台".to_string()),
            stock_code: Some("600519".to_string()),
            org_name: Some("华鑫证券有限责任公司".to_string()),
            org_s_name: Some("华鑫证券".to_string()),
            publish_date: Some("2026-05-05 00:00:00.000".to_string()),
            info_code: Some("AP202605051821970230".to_string()),
            researcher: Some("孙山山".to_string()),
            em_rating_name: Some("买入".to_string()),
            s_rating_name: None,
            industry_name: None,
            indv_indu_name: Some("白酒Ⅱ".to_string()),
            attach_pages: Some(serde_json::json!(5)),
        };

        let line = eastmoney_report_line(&row, EastmoneyReportScope::Stock).unwrap();
        assert!(line.contains("标的:贵州茅台(600519)"));
        assert!(line.contains("评级:买入"));
        assert!(
            line.contains(
                "详情页 https://data.eastmoney.com/report/info/AP202605051821970230.html"
            )
        );
        assert!(line.contains("PDF https://pdf.dfcfw.com/pdf/H3_AP202605051821970230_1.pdf"));
    }

    #[test]
    fn select_eastmoney_industry_code_prefers_exact_then_level_2() {
        let rows = vec![
            EastmoneyBoardRow {
                board_code: Some("1575".to_string()),
                board_name: Some("白酒Ⅲ".to_string()),
                board_level: Some("3".to_string()),
            },
            EastmoneyBoardRow {
                board_code: Some("1277".to_string()),
                board_name: Some("白酒Ⅱ".to_string()),
                board_level: Some("2".to_string()),
            },
            EastmoneyBoardRow {
                board_code: Some("1033".to_string()),
                board_name: Some("电池".to_string()),
                board_level: Some("2".to_string()),
            },
        ];

        assert_eq!(
            select_eastmoney_industry_code(&rows, "电池").as_deref(),
            Some("1033")
        );
        assert_eq!(
            select_eastmoney_industry_code(&rows, "白酒").as_deref(),
            Some("1277")
        );
    }
}
