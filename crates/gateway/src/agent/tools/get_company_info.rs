use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_company_info";
const ENDPOINT: &str = "https://api.tushare.pro";
const REQUEST_TIMEOUT_SECS: u64 = 20;

pub struct GetCompanyInfoTool {
    http: Client,
}

impl GetCompanyInfoTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;
        Ok(Self { http })
    }

    async fn tushare_post(
        &self,
        payload: serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value> {
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.post(ENDPOINT).json(&payload).send() => r?,
        };
        if !resp.status().is_success() {
            bail!("tushare returned HTTP {}", resp.status().as_u16());
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
                .unwrap_or("unknown tushare error");
            bail!("tushare error (code={code}): {msg}");
        }
        Ok(body)
    }
}

fn exchange_from_ts_code(ts_code: &str) -> &'static str {
    if ts_code.ends_with(".SH") {
        "SSE"
    } else if ts_code.ends_with(".SZ") {
        "SZSE"
    } else {
        "BSE"
    }
}

fn extract_field<'a>(fields: &[String], items: &'a [serde_json::Value], name: &str) -> &'a str {
    let idx = fields.iter().position(|f| f == name);
    idx.and_then(|i| items.get(i))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

fn extract_field_str(fields: &[String], items: &[serde_json::Value], name: &str) -> String {
    extract_field(fields, items, name).to_string()
}

#[async_trait]
impl ToolHandler for GetCompanyInfoTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch A-share company background and key financial indicators from Tushare. \
                Use when researching a company's business, industry, management, and financial health. \
                Requires TUSHARE_TOKEN env var. ts_code format: \"600519.SH\", \"000001.SZ\". \
                Returns company profile + latest period key ratios in one call."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ts_code": {
                        "type": "string",
                        "description": "Tushare ts_code, e.g. '600519.SH', '000001.SZ'"
                    }
                },
                "required": ["ts_code"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let token = std::env::var("TUSHARE_TOKEN").map_err(|_| {
            anyhow!("TUSHARE_TOKEN env var not set — A-share data unavailable. Get a token at https://tushare.pro/register")
        })?;
        let ts_code = args
            .get("ts_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'ts_code' argument"))?
            .trim()
            .to_uppercase();

        let exchange = exchange_from_ts_code(&ts_code);

        let company_payload = serde_json::json!({
            "api_name": "stock_company",
            "token": token,
            "params": {"ts_code": ts_code, "exchange": exchange},
            "fields": "ts_code,com_name,exchange,chairman,manager,secretary,reg_capital,setup_date,province,city,introduction,website,employees,main_business,business_scope"
        });
        let company_body = self.tushare_post(company_payload, &cancel).await?;

        let fina_payload = serde_json::json!({
            "api_name": "fina_indicator_vip",
            "token": token,
            "params": {"ts_code": ts_code, "limit": 1},
            "fields": "end_date,eps,bps,roe,roa,grossprofit_margin,debt_to_assets,current_ratio,quick_ratio,revenue_yoy,profit_yoy,pe_ttm,pb"
        });
        let fina_body = self.tushare_post(fina_payload, &cancel).await?;

        let company_data = company_body
            .get("data")
            .ok_or_else(|| anyhow!("missing data in stock_company response"))?;
        let company_fields: Vec<String> = company_data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let company_items: Vec<serde_json::Value> = company_data
            .get("items")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|row| row.as_array())
            .cloned()
            .unwrap_or_default();

        if company_items.is_empty() {
            bail!("no company data returned for {ts_code}");
        }

        let com_name = extract_field_str(&company_fields, &company_items, "com_name");
        let chairman = extract_field_str(&company_fields, &company_items, "chairman");
        let manager = extract_field_str(&company_fields, &company_items, "manager");
        let reg_capital = extract_field_str(&company_fields, &company_items, "reg_capital");
        let setup_date = extract_field_str(&company_fields, &company_items, "setup_date");
        let province = extract_field_str(&company_fields, &company_items, "province");
        let city = extract_field_str(&company_fields, &company_items, "city");
        let website = extract_field_str(&company_fields, &company_items, "website");
        let employees = extract_field_str(&company_fields, &company_items, "employees");
        let main_business = extract_field_str(&company_fields, &company_items, "main_business");
        let introduction = extract_field_str(&company_fields, &company_items, "introduction");

        let mut out = format!("## {com_name} ({ts_code})\n\n");
        out.push_str("**基本信息**\n");
        out.push_str(&format!("- 公司名：{com_name}\n"));
        if !reg_capital.is_empty() {
            out.push_str(&format!("- 注册资金：{reg_capital}万元\n"));
        }
        if !setup_date.is_empty() {
            let date_fmt = if setup_date.len() == 8 {
                format!(
                    "{}-{}-{}",
                    &setup_date[..4],
                    &setup_date[4..6],
                    &setup_date[6..]
                )
            } else {
                setup_date.clone()
            };
            out.push_str(&format!("- 成立日期：{date_fmt}\n"));
        }
        if !province.is_empty() || !city.is_empty() {
            out.push_str(&format!("- 所在地：{province}{city}\n"));
        }
        if !employees.is_empty() {
            out.push_str(&format!("- 员工数：{employees}\n"));
        }
        if !chairman.is_empty() {
            out.push_str(&format!("- 董事长：{chairman}\n"));
        }
        if !manager.is_empty() {
            out.push_str(&format!("- 总经理：{manager}\n"));
        }
        if !website.is_empty() {
            out.push_str(&format!("- 官网：{website}\n"));
        }
        if !main_business.is_empty() {
            out.push_str(&format!("- 主营业务：{main_business}\n"));
        }
        if !introduction.is_empty() {
            out.push_str(&format!("\n**公司简介**\n{introduction}\n"));
        }

        let fina_data = fina_body
            .get("data")
            .ok_or_else(|| anyhow!("missing data in fina_indicator_vip response"))?;
        let fina_fields: Vec<String> = fina_data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let fina_items: Vec<serde_json::Value> = fina_data
            .get("items")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|row| row.as_array())
            .cloned()
            .unwrap_or_default();

        if !fina_items.is_empty() {
            let end_date = extract_field_str(&fina_fields, &fina_items, "end_date");
            let end_date_fmt = if end_date.len() == 8 {
                format!("{}-{}-{}", &end_date[..4], &end_date[4..6], &end_date[6..])
            } else {
                end_date.clone()
            };

            out.push_str(&format!("\n**最新财务指标** ({end_date_fmt})\n"));
            out.push_str("| 指标 | 值 |\n");
            out.push_str("|------|-----|\n");

            let metrics = [
                ("eps", "EPS", ""),
                ("bps", "BPS", ""),
                ("roe", "ROE", "%"),
                ("roa", "ROA", "%"),
                ("grossprofit_margin", "毛利率", "%"),
                ("debt_to_assets", "资产负债率", "%"),
                ("current_ratio", "流动比率", ""),
                ("quick_ratio", "速动比率", ""),
                ("pe_ttm", "PE(TTM)", "x"),
                ("pb", "PB", "x"),
                ("revenue_yoy", "营收同比", "%"),
                ("profit_yoy", "净利润同比", "%"),
            ];

            for (field, label, suffix) in &metrics {
                let val = extract_field(&fina_fields, &fina_items, field);
                if !val.is_empty() && val != "None" {
                    if suffix == &"%" {
                        let f: f64 = val.parse().unwrap_or(0.0);
                        if *field == "revenue_yoy" || *field == "profit_yoy" {
                            out.push_str(&format!("| {} | {:+.2}% |\n", label, f));
                        } else {
                            out.push_str(&format!("| {} | {:.2}% |\n", label, f));
                        }
                    } else if suffix == &"x" {
                        let f: f64 = val.parse().unwrap_or(0.0);
                        out.push_str(&format!("| {} | {:.1}x |\n", label, f));
                    } else {
                        out.push_str(&format!("| {} | {} |\n", label, val));
                    }
                }
            }
        }

        out.push_str("\n_Source: Tushare Pro_\n");
        Ok(out)
    }
}
