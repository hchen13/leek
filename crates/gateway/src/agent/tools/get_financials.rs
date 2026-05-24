//! `get_financials` — quarterly / annual financial statements for one
//! A-share symbol. Vendor-neutral key surface: `revenue`, `net_profit`,
//! `roe`, etc.

use std::sync::Arc;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;
use crate::vendors::{Symbol, VendorFinancial, VendorRegistry};

const DEFAULT_STATEMENT: &str = "ratios";
const DEFAULT_PERIOD: &str = "year";
const DEFAULT_COUNT: usize = 5;
const MAX_COUNT: usize = 20;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "get_financials".into(),
        description: "Fetch financial statements for one A-share symbol. Pick a \
             statement type:\n\
             - 'income': revenue, gross/operating profit, net profit, EPS\n\
             - 'balance': total assets, liabilities, equity, cash\n\
             - 'cashflow': operating / investing / financing cash flow, free cash flow\n\
             - 'ratios' (default): ROE, ROA, gross margin, net margin, debt ratios\n\
             Pick a period grain ('quarter' or 'year') and a count (most-recent \
             N rows). Returns rows in newest-first order. Use when the user wants \
             fundamentals or when technical signals need context. Note: field \
             names are generic (e.g. 'net_profit', not any vendor's schema name)."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "A-share symbol (e.g. '600519.SH', '300750')."
                },
                "statement": {
                    "type": "string",
                    "enum": ["income", "balance", "cashflow", "ratios"],
                    "description": "Which statement to fetch. Default 'ratios'."
                },
                "period": {
                    "type": "string",
                    "enum": ["quarter", "year"],
                    "description": "Reporting grain. Default 'year' (annual)."
                },
                "count": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_COUNT,
                    "description": "How many most-recent rows to return (max 20, default 5)."
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "财务报表",
        result: ResultArtifact::Card("financials"),
        summary: |args| {
            let s = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
            let st = args
                .get("statement")
                .and_then(|v| v.as_str())
                .unwrap_or(DEFAULT_STATEMENT);
            format!("财报 · {} · {}", super::summary_snippet(s), st)
        },
    }
}

pub async fn run(vendors: &Arc<VendorRegistry>, args: &serde_json::Value) -> ToolOutcome {
    let Some(raw) = args.get("symbol").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("get_financials: missing required argument 'symbol'.");
    };
    let symbol = match Symbol::parse(raw) {
        Ok(s) => s,
        Err(e) => return ToolOutcome::error(format!("get_financials: {e}")),
    };
    let statement = args
        .get("statement")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_STATEMENT)
        .to_string();
    if !matches!(
        statement.as_str(),
        "income" | "balance" | "cashflow" | "ratios"
    ) {
        return ToolOutcome::error(format!(
            "get_financials: invalid statement '{statement}' (try income/balance/cashflow/ratios)."
        ));
    }
    let period = args
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_PERIOD)
        .to_string();
    if !matches!(period.as_str(), "quarter" | "year") {
        return ToolOutcome::error(format!(
            "get_financials: invalid period '{period}' (try quarter/year)."
        ));
    }
    let count = args
        .get("count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_COUNT);
    if count == 0 || count > MAX_COUNT {
        return ToolOutcome::error(format!(
            "get_financials: count must be in 1..={MAX_COUNT} (got {count})."
        ));
    }

    // Fundamentals have no real free fallback — only the token-backed
    // primary source ships in M3. If it fails the model gets a clear
    // note that a PDF-parse fallback is out of scope (per spec).
    let chain: Vec<(&str, &dyn VendorFinancial)> = vec![("primary", &*vendors.tushare)];
    let mut attempts: Vec<(&str, String)> = Vec::new();
    let mut served: Option<(crate::vendors::FinancialReport, &str, &'static str)> = None;
    for (tag, vendor) in &chain {
        match vendor
            .fetch_financials(&symbol, &statement, &period, count)
            .await
        {
            Ok(r) => {
                attempts.push((vendor.vendor_name(), "ok".into()));
                served = Some((r, *tag, vendor.vendor_name()));
                break;
            }
            Err(e) => {
                tracing::info!(
                    vendor = vendor.vendor_name(),
                    recoverable = e.recoverable,
                    error = %e.message,
                    "get_financials vendor attempt failed",
                );
                attempts.push((vendor.vendor_name(), e.message.clone()));
                if !e.recoverable {
                    break;
                }
            }
        }
    }
    let (report, tag, served_by) = match served {
        Some(t) => t,
        None => {
            let summary = attempts
                .iter()
                .map(|(v, m)| format!("{v}: {m}"))
                .collect::<Vec<_>>()
                .join("; ");
            return ToolOutcome::error(format!(
                "get_financials: all data sources failed ({summary}). Fundamentals \
                 currently have no public fallback — see the README's A-share \
                 section for how to configure the primary data source."
            ));
        }
    };

    // Compact prose: latest period + first metric value per metric key.
    let latest = report.rows.first();
    let summary_line = match latest {
        Some(row) => {
            let highlights = row
                .metrics
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{k}={}",
                        v.map(|n| format!("{n:.3}")).unwrap_or_else(|| "n/a".into())
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{symbol} {statement} ({period}) 最新期 {label}: {highlights}",
                symbol = report.symbol,
                label = row.label,
            )
        }
        None => format!(
            "{} {} ({}) 没有任何行数据",
            report.symbol, statement, period
        ),
    };
    let model_output = format!(
        "{summary_line}\n（{} 行历史数据完整返回，见 display_payload.rows）",
        report.rows.len()
    );

    let display_payload = serde_json::json!({
        "kind": "financials",
        "symbol": report.symbol,
        "statement": report.statement,
        "period": report.period,
        "rows": report.rows,
        "data_source": tag,
    });
    let debug_payload = serde_json::json!({
        "symbol_input": raw,
        "symbol_normalized": symbol.to_dotted(),
        "served_by_vendor": served_by,
        "attempts": attempts.iter().map(|(v, m)| serde_json::json!({"vendor": v, "result": m})).collect::<Vec<_>>(),
        "row_count": report.rows.len(),
    });
    ToolOutcome::ok(model_output, display_payload, debug_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_well_formed_and_vendor_neutral() {
        let s = spec();
        assert_eq!(s.name, "get_financials");
        let d = s.description.to_lowercase();
        for forbidden in ["tushare", "eastmoney"] {
            assert!(!d.contains(forbidden), "leaked '{forbidden}'");
        }
    }

    #[test]
    fn invalid_statement_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "statement": "everything" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("statement"));
    }

    #[test]
    fn count_out_of_range_is_structured_error() {
        let vendors = Arc::new(VendorRegistry::for_test());
        let out = futures::executor::block_on(run(
            &vendors,
            &serde_json::json!({ "symbol": "600519.SH", "count": 0 }),
        ));
        assert!(out.is_error);
    }
}
