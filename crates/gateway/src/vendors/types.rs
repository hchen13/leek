//! Vendor-neutral data shapes returned by every market tool. Field names
//! use generic financial terminology (`revenue`, `net_profit`, `roe`) —
//! never vendor-specific schema names (Tushare's `or_tot`, `n_income_attr_p`,
//! etc.). The mapping happens inside each vendor module.

use serde::{Deserialize, Serialize};

/// Normalized A-share symbol: numeric code + standardized exchange suffix.
/// `600519.SH`, `000001.SZ`. The parser accepts `600519`, `600519.sh`,
/// `sh600519`, etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    /// 6-digit numeric code.
    pub code: String,
    /// Uppercase exchange suffix: `SH` for Shanghai (600/601/603/605/688/900),
    /// `SZ` for Shenzhen (000/001/002/003/300/301/200).
    pub exchange: String,
}

impl Symbol {
    /// Render `<code>.<exchange>`.
    pub fn to_dotted(&self) -> String {
        format!("{}.{}", self.code, self.exchange)
    }

    /// Render the form some vendors want (Sina: `sh600519`).
    pub fn to_prefixed(&self) -> String {
        format!("{}{}", self.exchange.to_lowercase(), self.code)
    }

    /// Render the form EastMoney wants (`1.600519` = SH, `0.000001` = SZ).
    /// Internal helper kept on `Symbol` so callers don't have to redo the
    /// mapping per vendor module.
    pub fn to_eastmoney(&self) -> String {
        let prefix = if self.exchange == "SH" { 1 } else { 0 };
        format!("{}.{}", prefix, self.code)
    }

    /// Parse `600519`, `600519.SH`, `sh600519`, etc. into a normalized
    /// symbol. Exchange is inferred from the leading code digits when
    /// missing.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        if s.is_empty() {
            return Err("symbol is empty".into());
        }
        // sh600519 / sz000001
        if s.len() >= 8 {
            let (head, tail) = s.split_at(2);
            let head_low = head.to_ascii_lowercase();
            if (head_low == "sh" || head_low == "sz")
                && tail.chars().all(|c| c.is_ascii_digit())
                && tail.len() == 6
            {
                return Ok(Self {
                    code: tail.into(),
                    exchange: head.to_ascii_uppercase(),
                });
            }
        }
        // 600519.SH
        if let Some((code, ex)) = s.split_once('.') {
            if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
                let ex = ex.to_ascii_uppercase();
                if ex == "SH" || ex == "SZ" {
                    return Ok(Self {
                        code: code.into(),
                        exchange: ex,
                    });
                }
                return Err(format!(
                    "unknown exchange suffix '{ex}' (expected SH or SZ)"
                ));
            }
        }
        // bare 6-digit code — infer
        if s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()) {
            let exchange = match &s[..3] {
                // Shanghai mainboard / sci-tech / B-share
                "600" | "601" | "603" | "605" | "688" | "900" => "SH",
                // Shenzhen mainboard / SME / ChiNext / B-share
                "000" | "001" | "002" | "003" | "300" | "301" | "200" => "SZ",
                _ => {
                    return Err(format!(
                        "cannot infer exchange for code '{s}' (use 600519.SH form)"
                    ));
                }
            };
            return Ok(Self {
                code: s.into(),
                exchange: exchange.into(),
            });
        }
        Err(format!(
            "'{raw}' is not a valid A-share symbol (try 600519.SH or 600519)"
        ))
    }
}

/// `market_quote` output — a snapshot of trading state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketQuote {
    pub symbol: String,
    pub name: Option<String>,
    /// Latest traded price (yuan).
    pub price: f64,
    /// Price change since the previous close (yuan, signed).
    pub change: f64,
    /// Percent change since the previous close (signed; `1.23` = +1.23%).
    pub change_pct: f64,
    /// Day-cumulative volume (shares).
    pub volume: Option<f64>,
    /// Day-cumulative turnover (yuan).
    pub turnover: Option<f64>,
    /// Open / high / low for the trading day.
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    /// Previous trading day's close.
    pub prev_close: Option<f64>,
    /// As-of timestamp in ISO 8601. Reflects when the upstream snapshot
    /// was generated — not the moment leek fetched it.
    pub timestamp: String,
    /// `"realtime"` / `"delayed"` / `"close"` (post-session snapshot).
    /// Tools that only return EoD prices report `"close"`.
    pub data_freshness: String,
}

/// `get_candlesticks` output — a single OHLCV bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    /// ISO 8601 date (`2026-05-20`) or compact (`20260520`); we normalize
    /// to ISO.
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub turnover: Option<f64>,
}

/// `get_financials` output — a list of reports across periods, in a
/// generic shape (no vendor-specific schema names). Selected statement
/// type is echoed back so the model knows what `metrics` means.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinancialReport {
    pub symbol: String,
    /// `"income"` / `"balance"` / `"cashflow"` / `"ratios"`.
    pub statement: String,
    /// `"quarter"` / `"year"`.
    pub period: String,
    pub rows: Vec<FinancialRow>,
}

/// One reporting-period worth of financial fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinancialRow {
    /// Period end date (ISO 8601). For annuals: `2024-12-31`.
    pub period_end: String,
    /// Period label, e.g. `2024Q4` / `2024`.
    pub label: String,
    /// Vendor-neutral metric name → value. Only fields the chosen
    /// statement covers are populated. Common keys:
    /// - income: `revenue`, `gross_profit`, `operating_profit`,
    ///   `net_profit`, `eps_basic`
    /// - balance: `total_assets`, `total_liabilities`, `total_equity`,
    ///   `cash_and_equivalents`
    /// - cashflow: `cf_operating`, `cf_investing`, `cf_financing`,
    ///   `free_cash_flow`
    /// - ratios: `roe`, `roa`, `gross_margin`, `net_margin`,
    ///   `debt_to_equity`, `current_ratio`, `quick_ratio`
    pub metrics: std::collections::BTreeMap<String, Option<f64>>,
}

/// `get_company_info` output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyInfo {
    pub symbol: String,
    pub name: String,
    pub industry: Option<String>,
    /// Chinese / SASAC sector classification (e.g. `白酒`).
    pub sector: Option<String>,
    /// Listing exchange display name (e.g. `上海证券交易所`).
    pub exchange: Option<String>,
    pub list_date: Option<String>,
    pub business_scope: Option<String>,
    /// Latest indicators snapshot, vendor-neutral keys: `market_cap`,
    /// `float_market_cap`, `pe_ttm`, `pb`, `dividend_yield_pct`.
    pub latest_indicators: std::collections::BTreeMap<String, Option<f64>>,
}

/// `get_capital_flow` output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapitalFlow {
    pub symbol: String,
    /// Aggregation window: `"1d"` / `"5d"` / `"20d"`.
    pub period: String,
    /// Net inflow over the window (yuan, signed).
    pub net_inflow: f64,
    /// Retail-classified net flow.
    pub retail_flow: Option<f64>,
    /// Institutional / large-trade net flow ("main force" in CN parlance).
    pub main_flow: Option<f64>,
    /// Northbound (Stock Connect) flow availability — `false` when the
    /// vendor quota doesn't include it. The `north_flow` field stays
    /// `None` in that case so the model never reads a half-truth value.
    pub north_flow_available: bool,
    /// Northbound net flow (yuan, signed). `None` whenever
    /// `north_flow_available == false`.
    pub north_flow: Option<f64>,
    /// Any partial-data warnings (e.g. "northbound quota exhausted").
    pub warnings: Vec<String>,
}

// ── M4.1 — supplementary research data shapes ───────────────────────────
//
// Each new tool gets one struct; field names are vendor-neutral (no
// `or_tot`-style upstream schema leaks). When a vendor refuses to serve
// (free-tier quota, paywalled endpoint, dead selector), the per-tool
// dispatch surfaces `data_available: false` + a reason — see the tool
// modules. The structs below only carry positive data.

/// `get_industry_peers` output — one peer per row, target included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndustryPeers {
    pub symbol: String,
    /// Industry classification used to choose peers.
    pub industry: String,
    /// Optional finer classification (sub-industry) when available.
    pub sub_industry: Option<String>,
    /// One of `"valuation"` / `"growth"` / `"profitability"`.
    pub dimension: String,
    /// One row per peer — first row is always the target symbol.
    pub peers: Vec<PeerRow>,
    /// Median of the principal metric across peers; key matches
    /// `principal_metric` so the model knows what "median" refers to.
    pub principal_metric: String,
    pub median: Option<f64>,
    /// 0..1 quantile of the target relative to the peer set on
    /// `principal_metric` (`None` if data is too sparse).
    pub target_quantile: Option<f64>,
}

/// One company row in an industry-peer comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRow {
    pub symbol: String,
    pub name: String,
    /// Metric name → value. Keys depend on dimension; see
    /// `get_industry_peers` tool doc.
    pub metrics: std::collections::BTreeMap<String, Option<f64>>,
    /// `true` if this row is the originally requested company.
    pub is_target: bool,
}

/// `get_business_breakdown` output — main-business revenue split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessBreakdown {
    pub symbol: String,
    /// Reporting period the breakdown applies to (ISO 8601 date or
    /// vendor's `YYYYMMDD` echoed back).
    pub period_end: String,
    /// `"product"` / `"industry"` / `"region"`.
    pub dimension: String,
    pub items: Vec<BusinessBreakdownRow>,
}

/// One slice of the main-business breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessBreakdownRow {
    /// Label of the slice (product name / industry / region).
    pub item: String,
    /// Revenue in yuan.
    pub revenue_yuan: Option<f64>,
    /// Slice as a percentage of total revenue (0..100).
    pub pct_of_total: Option<f64>,
    /// Gross margin for the slice in percent (0..100).
    pub gross_margin_pct: Option<f64>,
    /// Year-over-year revenue change in percent.
    pub yoy_change_pct: Option<f64>,
}

/// `get_announcements` output — recent corporate announcements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnouncementList {
    pub symbol: String,
    /// Day window the list covers.
    pub days: usize,
    /// Optional category filter the caller supplied.
    pub category_filter: Option<String>,
    pub items: Vec<AnnouncementItem>,
}

/// One announcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnouncementItem {
    /// ISO 8601 publication date.
    pub date: String,
    pub title: String,
    /// Best-effort category tag (e.g. `重大事项` / `分红配股`). May be
    /// empty when the vendor doesn't classify.
    pub category: Option<String>,
    /// Direct PDF / HTML URL when available.
    pub url: Option<String>,
    /// Short abstract / summary line when available.
    pub abstract_text: Option<String>,
}

/// `get_consensus` output — sell-side consensus forecasts + rating mix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalystConsensus {
    pub symbol: String,
    /// Year → forecast aggregate for that fiscal year (e.g. `2025`,
    /// `2026`). Sorted by year ascending.
    pub forecasts: Vec<ConsensusForecast>,
    /// Rating distribution from analyst reports in a recent window
    /// (typically the last 90 days when the vendor exposes one).
    pub rating_mix: ConsensusRatingMix,
    /// Number of distinct broker reports the rating mix is derived from.
    pub report_count: Option<usize>,
}

/// One year of consensus forecast aggregates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusForecast {
    pub year: String,
    pub revenue_mean_yuan: Option<f64>,
    pub revenue_high_yuan: Option<f64>,
    pub revenue_low_yuan: Option<f64>,
    pub net_profit_mean_yuan: Option<f64>,
    pub net_profit_high_yuan: Option<f64>,
    pub net_profit_low_yuan: Option<f64>,
    pub eps_mean: Option<f64>,
    pub eps_high: Option<f64>,
    pub eps_low: Option<f64>,
}

/// Aggregate count by rating bucket.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsensusRatingMix {
    pub buy: usize,
    pub overweight: usize,
    pub hold: usize,
    pub underweight: usize,
    pub sell: usize,
}

/// `get_top_holders` output — top-10 holder list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopHolders {
    pub symbol: String,
    /// `"total"` (all shareholders, top 10) or `"float"` (top 10 of the
    /// floating share register only).
    pub kind: String,
    /// Reporting period end date (ISO 8601) the list applies to.
    pub period_end: String,
    pub holders: Vec<HolderRow>,
}

/// One holder row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolderRow {
    pub rank: u32,
    pub holder_name: String,
    /// Number of shares held.
    pub shares: Option<f64>,
    /// Percent of total / floating shares (0..100, matches `kind`).
    pub pct: Option<f64>,
    /// Change vs the previous reporting period in shares (signed).
    /// `None` when the vendor doesn't expose the prior period.
    pub change_qoq_shares: Option<f64>,
}

/// `get_concepts` output — concept / theme tags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptList {
    pub symbol: String,
    pub concepts: Vec<ConceptTag>,
}

/// One concept tag with an optional popularity rank.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptTag {
    pub name: String,
    /// Source-specific code, if any.
    pub code: Option<String>,
    /// `1` = hottest; smaller = hotter. `None` when the vendor doesn't
    /// rank concepts.
    pub heat_rank: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dotted_symbol() {
        let s = Symbol::parse("600519.SH").unwrap();
        assert_eq!(s.code, "600519");
        assert_eq!(s.exchange, "SH");
        assert_eq!(s.to_dotted(), "600519.SH");
        assert_eq!(s.to_prefixed(), "sh600519");
        assert_eq!(s.to_eastmoney(), "1.600519");
    }

    #[test]
    fn parses_lowercase_dotted_symbol() {
        let s = Symbol::parse("000001.sz").unwrap();
        assert_eq!(s.exchange, "SZ");
        assert_eq!(s.to_eastmoney(), "0.000001");
    }

    #[test]
    fn parses_prefixed_symbol() {
        let s = Symbol::parse("sh600519").unwrap();
        assert_eq!(s.code, "600519");
        assert_eq!(s.exchange, "SH");
    }

    #[test]
    fn infers_exchange_from_bare_code() {
        assert_eq!(Symbol::parse("600519").unwrap().exchange, "SH"); // mainboard
        assert_eq!(Symbol::parse("000001").unwrap().exchange, "SZ"); // Shenzhen
        assert_eq!(Symbol::parse("300750").unwrap().exchange, "SZ"); // ChiNext
        assert_eq!(Symbol::parse("688981").unwrap().exchange, "SH"); // STAR
        assert_eq!(Symbol::parse("605378").unwrap().exchange, "SH"); // SH mainboard 605
    }

    #[test]
    fn rejects_garbage() {
        assert!(Symbol::parse("").is_err());
        assert!(Symbol::parse("foo").is_err());
        assert!(Symbol::parse("123").is_err()); // wrong length
        assert!(Symbol::parse("999999").is_err()); // unknown prefix
        assert!(Symbol::parse("600519.XX").is_err());
    }
}
