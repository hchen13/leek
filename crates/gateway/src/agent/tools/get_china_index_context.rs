use std::time::Duration;

use anyhow::{bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::{
    llm::ToolSpec,
    tushare::{TushareClient, TushareResponse},
};

use super::{data_provider_tokens, ToolContext, ToolHandler};

const TOOL_NAME: &str = "get_china_index_context";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 30;

pub struct GetChinaIndexContextTool {
    http: Client,
}

impl GetChinaIndexContextTool {
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
impl ToolHandler for GetChinaIndexContextTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Build China index context from configured index-market data sources for mainland exchange/CSI/SW indexes and THS concept/industry indexes: index profile, recent quote, valuation/basic metrics, and constituent weights when available. Use this for A-share benchmark, sector, ETF-underlying, or peer-basket context."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "index_code": {
                        "type": "string",
                        "description": "Optional index code, e.g. 000300.SH or 885710.TI. Without it, returns an index universe sample for the selected market."
                    },
                    "market": {
                        "type": "string",
                        "description": "Index market/source. Common values: SSE, SZSE, CSI, SW, CNI, OTH, THS. Default inferred from index_code, otherwise CSI."
                    },
                    "sections": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["profile", "quote", "valuation", "weights"]
                        },
                        "description": "Default with index_code: profile, quote, valuation, weights. Without index_code: profile only."
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
        let index_code = args
            .get("index_code")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty());
        let market = args
            .get("market")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_uppercase)
            .unwrap_or_else(|| infer_market(index_code.as_deref()).to_string());
        let is_ths = is_ths_index(index_code.as_deref(), &market);
        let sections = sections(&args, index_code.is_some());
        let limit = limit(&args);

        if index_code.is_none() && sections.iter().any(|s| s != "profile") {
            bail!("index_code is required for quote/valuation/weights");
        }

        let mut out = match index_code.as_deref() {
            Some(code) => format!("## 中国指数上下文 · {code}\n\n"),
            None => format!("## 中国指数池样本 · market={market}\n\n"),
        };

        if sections.iter().any(|s| s == "profile") {
            let profile = if is_ths {
                self.tushare_post(
                    &token,
                    "ths_index",
                    ths_index_params(index_code.as_deref()),
                    "ts_code,name,count,exchange,list_date,type",
                    &cancel,
                )
                .await?
            } else {
                self.tushare_post(
                    &token,
                    "index_basic",
                    index_basic_params(index_code.as_deref(), &market),
                    "ts_code,name,market,publisher,category,base_date,base_point,list_date,weight_rule,desc,exp_date",
                    &cancel,
                )
                .await?
            };
            out.push_str(&render_profile(&profile, is_ths, limit));
        }

        if let Some(code) = index_code.as_deref() {
            if sections.iter().any(|s| s == "quote") {
                let quote = if is_ths {
                    self.tushare_post(
                        &token,
                        "ths_daily",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,trade_date,close,open,high,low,pre_close,avg_price,change,pct_change,vol,turnover_rate,total_mv,float_mv",
                        &cancel,
                    )
                    .await?
                } else {
                    self.tushare_post(
                        &token,
                        "index_daily",
                        serde_json::json!({"ts_code": code, "limit": limit}),
                        "ts_code,trade_date,close,open,high,low,pre_close,change,pct_chg,vol,amount",
                        &cancel,
                    )
                    .await?
                };
                out.push_str(&render_quote(&quote, is_ths));
            }

            if sections.iter().any(|s| s == "valuation") {
                if is_ths {
                    out.push_str("### 规模与换手 · 同花顺指数行情\n");
                    out.push_str("- 当前同花顺指数没有独立估值口径；已在行情段返回总市值、流通市值和换手率。\n\n");
                } else {
                    let valuation = self
                        .tushare_post(
                            &token,
                            "index_dailybasic",
                            serde_json::json!({"ts_code": code, "limit": limit}),
                            "ts_code,trade_date,total_mv,float_mv,total_share,float_share,free_share,turnover_rate,turnover_rate_f,pe,pe_ttm,pb",
                            &cancel,
                        )
                        .await?;
                    out.push_str(&render_valuation(&valuation));
                }
            }

            if sections.iter().any(|s| s == "weights") {
                if is_ths {
                    out.push_str("### 权重/成分 · 同花顺指数\n");
                    out.push_str("- 当前工具未承诺同花顺指数成分权重。需要成分时，用普通指数权重、行业工具或后续接入同花顺成分来源。\n\n");
                } else {
                    let weights = self
                        .tushare_post(
                            &token,
                            "index_weight",
                            serde_json::json!({"index_code": code, "limit": limit}),
                            "index_code,con_code,trade_date,weight",
                            &cancel,
                        )
                        .await?;
                    out.push_str(&render_weights(&weights));
                }
            }
        }

        out.push_str("_Caveat: 指数工具给出 benchmark/行业/组合上下文，不直接生成交易结论。若用于 ETF 分析，应继续结合 ETF 费率、跟踪误差、流动性和折溢价。_");
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

fn index_basic_params(index_code: Option<&str>, market: &str) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    params.insert("market".to_string(), serde_json::json!(market));
    if let Some(code) = index_code {
        params.insert("ts_code".to_string(), serde_json::json!(code));
    }
    serde_json::Value::Object(params)
}

fn ths_index_params(index_code: Option<&str>) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(code) = index_code {
        params.insert("ts_code".to_string(), serde_json::json!(code));
    } else {
        params.insert("type".to_string(), serde_json::json!("N"));
    }
    serde_json::Value::Object(params)
}

fn infer_market(index_code: Option<&str>) -> &'static str {
    match index_code {
        Some(code) if code.ends_with(".TI") => "THS",
        Some(code) if code.ends_with(".SI") => "SW",
        Some(code) if code.ends_with(".CSI") => "CSI",
        Some(code) if code.ends_with(".SZ") => "SZSE",
        Some(code) if code.ends_with(".SH") => "SSE",
        _ => "CSI",
    }
}

fn is_ths_index(index_code: Option<&str>, market: &str) -> bool {
    market.eq_ignore_ascii_case("THS")
        || index_code
            .map(|code| code.ends_with(".TI"))
            .unwrap_or(false)
}

fn sections(args: &serde_json::Value, has_index_code: bool) -> Vec<String> {
    args.get("sections")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            if has_index_code {
                vec![
                    "profile".to_string(),
                    "quote".to_string(),
                    "valuation".to_string(),
                    "weights".to_string(),
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

fn render_profile(table: &Table, is_ths: bool, limit: usize) -> String {
    let api = if is_ths { "ths_index" } else { "index_basic" };
    let mut out = section_header("指数资料", api, table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No index profile rows returned. Check index_code/market.\n\n");
        return out;
    }
    for row in table.rows.iter().take(limit) {
        if is_ths {
            out.push_str(&format!(
                "- {} {} · 成分数={} · exchange={} · list={}\n",
                table.text(row, "ts_code"),
                table.text(row, "name"),
                table.text(row, "count"),
                table.text(row, "exchange"),
                fmt_date(&table.text(row, "list_date"))
            ));
        } else {
            out.push_str(&format!(
                "- {} {} · {} · 发布方={} · 基日={} · 基点={} · 权重={}\n  {}\n",
                table.text(row, "ts_code"),
                table.text(row, "name"),
                table.text(row, "category"),
                table.text(row, "publisher"),
                fmt_date(&table.text(row, "base_date")),
                table.text(row, "base_point"),
                table.text(row, "weight_rule"),
                compact_text(&table.text(row, "desc"), 120)
            ));
        }
    }
    out.push('\n');
    out
}

fn render_quote(table: &Table, is_ths: bool) -> String {
    let api = if is_ths { "ths_daily" } else { "index_daily" };
    let mut out = section_header("近期表现", api, table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No index quote rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(8) {
        let pct_field = if is_ths { "pct_change" } else { "pct_chg" };
        out.push_str(&format!(
            "- {} · close={} · pct={}% · amount/vol={}{}\n",
            fmt_date(&table.text(row, "trade_date")),
            table.text(row, "close"),
            table.text(row, pct_field),
            if is_ths {
                table.text(row, "vol")
            } else {
                table.text(row, "amount")
            },
            if is_ths {
                format!(
                    " · turnover={} · total_mv={}",
                    table.text(row, "turnover_rate"),
                    table.text(row, "total_mv")
                )
            } else {
                String::new()
            }
        ));
    }
    out.push('\n');
    out
}

fn render_valuation(table: &Table) -> String {
    let mut out = section_header("估值与成交", "index_dailybasic", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No valuation/basic rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(8) {
        out.push_str(&format!(
            "- {} · PE={} · PE_TTM={} · PB={} · turnover={} · total_mv={}\n",
            fmt_date(&table.text(row, "trade_date")),
            table.text(row, "pe"),
            table.text(row, "pe_ttm"),
            table.text(row, "pb"),
            table.text(row, "turnover_rate"),
            table.text(row, "total_mv")
        ));
    }
    out.push('\n');
    out
}

fn render_weights(table: &Table) -> String {
    let mut out = section_header("成分权重", "index_weight", table.rows.len());
    if table.rows.is_empty() {
        out.push_str("- No constituent weight rows returned.\n\n");
        return out;
    }
    for row in table.rows.iter().take(15) {
        out.push_str(&format!(
            "- {} · {} · weight={}%\n",
            fmt_date(&table.text(row, "trade_date")),
            table.text(row, "con_code"),
            table.text(row, "weight")
        ));
    }
    out.push('\n');
    out
}

fn section_header(title: &str, api: &str, rows: usize) -> String {
    format!("### {title} · {} · {rows}条\n", index_source_label(api))
}

fn index_source_label(api: &str) -> &'static str {
    match api {
        "index_basic" => "指数资料",
        "ths_index" => "同花顺指数资料",
        "index_daily" => "指数日频行情",
        "ths_daily" => "同花顺指数行情",
        "index_dailybasic" => "指数估值与成交",
        "index_weight" => "指数成分权重",
        _ => "结构化指数来源",
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
    fn infers_market_from_index_code() {
        assert_eq!(infer_market(Some("885710.TI")), "THS");
        assert_eq!(infer_market(Some("801120.SI")), "SW");
        assert_eq!(infer_market(Some("000300.SH")), "SSE");
        assert_eq!(infer_market(Some("399001.SZ")), "SZSE");
        assert_eq!(infer_market(None), "CSI");
    }

    #[test]
    fn detects_ths_index() {
        assert!(is_ths_index(Some("885710.TI"), "CSI"));
        assert!(is_ths_index(None, "THS"));
        assert!(!is_ths_index(Some("000300.SH"), "SSE"));
    }

    #[test]
    fn defaults_sections_by_code_presence() {
        assert_eq!(sections(&serde_json::json!({}), false), vec!["profile"]);
        let with_code = sections(&serde_json::json!({}), true);
        assert!(with_code.contains(&"quote".to_string()));
        assert!(with_code.contains(&"weights".to_string()));
    }

    #[test]
    fn renders_empty_weights_as_data_gap() {
        let table = Table {
            fields: vec!["trade_date".into()],
            rows: Vec::new(),
        };
        let out = render_weights(&table);
        assert!(out.contains("No constituent weight rows returned"));
    }
}
