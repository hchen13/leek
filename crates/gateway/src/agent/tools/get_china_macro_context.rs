use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{data_provider_tokens, ToolContext, ToolHandler};

const TOOL_NAME: &str = "get_china_macro_context";
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub struct GetChinaMacroContextTool {
    http: Client,
}

impl GetChinaMacroContextTool {
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
impl ToolHandler for GetChinaMacroContextTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch China macro context from configured macro data sources: growth, inflation, liquidity/credit, rates, PMI, and economic release calendar. Use only when the user's investment question needs a macro transmission chain; do not use it as generic decoration. Returns latest points and caveats, not forecasts."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "topics": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["growth", "inflation", "liquidity", "rates", "pmi", "calendar"]
                        },
                        "description": "Default: growth, inflation, liquidity, rates, pmi, calendar"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Rows per series, default 4, max 12"
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
        let topics = topics(&args);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, 12))
            .unwrap_or(4);

        let mut sections = Vec::new();
        if topics.iter().any(|t| t == "growth") {
            sections.push(
                self.latest_section(
                    &token,
                    "GDP",
                    "cn_gdp",
                    serde_json::json!({}),
                    "quarter,gdp,yoy,pi,si,ti",
                    limit,
                    &cancel,
                )
                .await?,
            );
        }
        if topics.iter().any(|t| t == "inflation") {
            sections.push(
                self.latest_section(
                    &token,
                    "CPI",
                    "cn_cpi",
                    serde_json::json!({}),
                    "month,nt_val,nt_yoy,town_val,town_yoy,cnt_val,cnt_yoy",
                    limit,
                    &cancel,
                )
                .await?,
            );
            sections.push(
                self.latest_section(
                    &token,
                    "PPI",
                    "cn_ppi",
                    serde_json::json!({}),
                    "month,ppi_yoy,ppi_mp_yoy,ppi_ppi_yoy",
                    limit,
                    &cancel,
                )
                .await?,
            );
        }
        if topics.iter().any(|t| t == "liquidity") {
            sections.push(
                self.latest_section(
                    &token,
                    "M2 / 社融候选",
                    "cn_m",
                    serde_json::json!({}),
                    "month,m2,m2_yoy,m1,m1_yoy,m0,m0_yoy",
                    limit,
                    &cancel,
                )
                .await?,
            );
            sections.push(
                self.latest_section(
                    &token,
                    "社融",
                    "sf_month",
                    serde_json::json!({}),
                    "month,inc_month,inc_cum,stk_end",
                    limit,
                    &cancel,
                )
                .await?,
            );
        }
        if topics.iter().any(|t| t == "rates") {
            sections.push(
                self.latest_section(
                    &token,
                    "LPR",
                    "shibor_lpr",
                    serde_json::json!({}),
                    "date,1y,5y",
                    limit,
                    &cancel,
                )
                .await?,
            );
            sections.push(
                self.latest_section(
                    &token,
                    "Shibor",
                    "shibor",
                    serde_json::json!({}),
                    "date,on,1w,1m,3m,6m,1y",
                    limit,
                    &cancel,
                )
                .await?,
            );
        }
        if topics.iter().any(|t| t == "pmi") {
            sections.push(
                self.latest_section(
                    &token,
                    "PMI",
                    "cn_pmi",
                    serde_json::json!({}),
                    "month,pmi010000,pmi010100,pmi010200,pmi010300,pmi010400",
                    limit,
                    &cancel,
                )
                .await?,
            );
        }
        if topics.iter().any(|t| t == "calendar") {
            sections.push(
                self.latest_section(
                    &token,
                    "宏观日程",
                    "cn_schedule",
                    serde_json::json!({}),
                    "date,indicator,period,forecast,actual,previous,importance",
                    limit,
                    &cancel,
                )
                .await?,
            );
        }

        let mut out = "## 中国宏观上下文\n\n".to_string();
        for section in sections {
            out.push_str(&section);
            out.push('\n');
        }
        out.push_str("_Caveat: 宏观数据必须通过需求、价格、利率、信用或政策传导链连接到标的，不应直接替代公司与行业事实。_");
        Ok(out)
    }
}

impl GetChinaMacroContextTool {
    #[allow(clippy::too_many_arguments)]
    async fn latest_section(
        &self,
        token: &str,
        title: &str,
        api_name: &str,
        params: serde_json::Value,
        fields: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let table = self
            .tushare_post(token, api_name, params, fields, cancel)
            .await?;
        let mut out = format!(
            "### {} · {} · {}条\n",
            title,
            macro_source_label(api_name),
            table.rows.len()
        );
        if table.rows.is_empty() {
            out.push_str("- 当前宏观来源没有返回记录；不要从缺口本身推断宏观信号。\n");
            return Ok(out);
        }
        for row in table.rows.iter().take(limit) {
            out.push_str("- ");
            out.push_str(&table.row_summary(row));
            out.push('\n');
        }
        Ok(out)
    }
}

fn macro_source_label(api_name: &str) -> &'static str {
    match api_name {
        "cn_gdp" => "GDP/产业增加值",
        "cn_cpi" => "居民价格指数",
        "cn_ppi" => "工业品价格指数",
        "cn_m" => "货币供应",
        "sf_month" => "社会融资",
        "shibor_lpr" => "LPR",
        "shibor" => "Shibor",
        "cn_pmi" => "采购经理指数",
        "cn_schedule" => "宏观日程",
        _ => "宏观数据",
    }
}

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

    fn row_summary(&self, row: &[serde_json::Value]) -> String {
        self.fields
            .iter()
            .enumerate()
            .filter_map(|(i, field)| row.get(i).map(|value| (field, value_text(value))))
            .filter(|(_, value)| !value.is_empty())
            .map(|(field, value)| format!("{}={value}", macro_field_label(field)))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn macro_field_label(field: &str) -> &str {
    match field {
        "quarter" => "季度",
        "month" => "月份",
        "date" => "日期",
        "gdp" => "GDP",
        "yoy" => "同比",
        "pi" => "第一产业",
        "si" => "第二产业",
        "ti" => "第三产业",
        "nt_val" => "全国CPI",
        "nt_yoy" => "全国CPI同比",
        "town_val" => "城市CPI",
        "town_yoy" => "城市CPI同比",
        "cnt_val" => "农村CPI",
        "cnt_yoy" => "农村CPI同比",
        "ppi_yoy" => "PPI同比",
        "ppi_mp_yoy" => "生产资料同比",
        "ppi_ppi_yoy" => "生活资料同比",
        "m2" => "M2",
        "m2_yoy" => "M2同比",
        "m1" => "M1",
        "m1_yoy" => "M1同比",
        "m0" => "M0",
        "m0_yoy" => "M0同比",
        "inc_month" => "当月新增",
        "inc_cum" => "累计新增",
        "stk_end" => "期末存量",
        "1y" => "1年",
        "5y" => "5年",
        "on" => "隔夜",
        "1w" => "1周",
        "1m" => "1月",
        "3m" => "3月",
        "6m" => "6月",
        "pmi010000" => "制造业PMI",
        "pmi010100" => "生产",
        "pmi010200" => "新订单",
        "pmi010300" => "新出口订单",
        "pmi010400" => "在手订单",
        "indicator" => "指标",
        "period" => "周期",
        "forecast" => "预测",
        "actual" => "实际",
        "previous" => "前值",
        "importance" => "重要性",
        _ => field,
    }
}

fn topics(args: &serde_json::Value) -> Vec<String> {
    args.get("topics")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            vec![
                "growth".to_string(),
                "inflation".to_string(),
                "liquidity".to_string(),
                "rates".to_string(),
                "pmi".to_string(),
                "calendar".to_string(),
            ]
        })
}

fn value_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_all_macro_topics() {
        let ts = topics(&serde_json::json!({}));
        assert!(ts.contains(&"growth".to_string()));
        assert!(ts.contains(&"calendar".to_string()));
    }

    #[test]
    fn row_summary_skips_nulls() {
        let table = Table {
            fields: vec!["month".into(), "m2_yoy".into(), "empty".into()],
            rows: vec![],
        };
        let row = vec![
            serde_json::json!("202604"),
            serde_json::json!(7.2),
            serde_json::Value::Null,
        ];
        let summary = table.row_summary(&row);
        assert!(summary.contains("月份=202604"));
        assert!(summary.contains("M2同比=7.2"));
        assert!(!summary.contains("empty="));
    }
}
