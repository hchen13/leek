//! Vendor-neutral shapes shared across the M4.1.1 facts-only tool set.
//!
//! Field names use generic financial terminology only (e.g. `revenue`,
//! `pe_ttm`, `roe`) — never an upstream's schema field name. Vendor
//! identity is confined to the `vendors/` module bodies, env-var names
//! and tracing log fields. The `tests/m3_vendor_neutrality.rs`
//! integration test enforces that nothing leaks across the model
//! boundary.
//!
//! Most of these types are *transport* shapes — the tools collapse them
//! into a per-call display payload and a distilled markdown surface.
//! They are kept narrow and serializable so the canvas card can read
//! them verbatim.
//!
//! ## Date handling
//!
//! All `date` / `period_end` strings are ISO 8601 (`2026-05-26`). The
//! vendor parsers normalize compact dates (`20260526`) inside their
//! adapter modules.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Symbol ────────────────────────────────────────────────────────────

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

    /// Render the form some vendors want (Sina-style: `sh600519`).
    #[allow(dead_code)]
    pub fn to_prefixed(&self) -> String {
        format!("{}{}", self.exchange.to_lowercase(), self.code)
    }

    /// Render the form EastMoney's push2 + datacenter accept
    /// (`1.600519` = SH, `0.000001` = SZ).
    pub fn to_eastmoney(&self) -> String {
        let prefix = if self.exchange == "SH" { 1 } else { 0 };
        format!("{}.{}", prefix, self.code)
    }

    /// Parse `600519`, `600519.SH`, `sh600519`, etc. Exchange is inferred
    /// from the leading code digits when missing.
    ///
    /// **UTF-8 safety:** `raw` may originate from agent input or vendor
    /// fields that legitimately carry CJK strings (industry names like
    /// `化学制药`, company names like `恒瑞医药`). The byte-indexing
    /// branches below MUST be reached only after we confirm the input is
    /// pure ASCII — otherwise `split_at(2)` panics inside a multi-byte
    /// character (`thread 'tokio-rt-worker' panicked at ... byte index 2
    /// is not a char boundary; it is inside '化'`). Callers like
    /// `industry_landscape` deliberately try `parse` on industry names
    /// and fall back when it returns `Err`, so a non-ASCII input here is
    /// a structured failure, not a panic.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        if s.is_empty() {
            return Err("symbol is empty".into());
        }
        // All branches below assume an ASCII layout (letters + digits +
        // a single dot). Non-ASCII input cannot be a symbol — bail fast
        // so byte indexing later is safe.
        if !s.is_ascii() {
            return Err(format!(
                "'{raw}' is not a valid A-share symbol (try 600519.SH or 600519)"
            ));
        }
        // sh600519 / sz000001 — `is_ascii` above guarantees `s.len()` is
        // the char count, so `split_at(2)` lands on a char boundary.
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
        // Bare 6-digit code — infer.
        if s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()) {
            let exchange = match &s[..3] {
                "600" | "601" | "603" | "605" | "688" | "900" => "SH",
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

// ── Vendor error envelope ─────────────────────────────────────────────

/// One vendor's reason for not serving a request. `recoverable=true`
/// means a sibling endpoint or fallback may still help; the tools surface
/// recoverable failures as `empty_dimensions` (still success) when a
/// dimension is optional, or as a structured tool error otherwise.
#[derive(Debug, Clone)]
pub struct VendorError {
    #[allow(dead_code)]
    pub vendor: &'static str,
    pub message: String,
    /// Hint kept on the struct for callers that want to know whether
    /// they should attempt a sibling endpoint. Tools mostly route on
    /// `message`, so the field reads as dead-code without an explicit
    /// allow.
    #[allow(dead_code)]
    pub recoverable: bool,
}

impl VendorError {
    pub fn recoverable(vendor: &'static str, message: impl Into<String>) -> Self {
        Self {
            vendor,
            message: message.into(),
            recoverable: true,
        }
    }
    pub fn fatal(vendor: &'static str, message: impl Into<String>) -> Self {
        Self {
            vendor,
            message: message.into(),
            recoverable: false,
        }
    }
}

impl std::fmt::Display for VendorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.vendor, self.message)
    }
}

impl std::error::Error for VendorError {}

// ── Macro indicators (tool 1) ─────────────────────────────────────────

/// One observation in a monthly CPI / PPI series.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpiPpiObs {
    /// `YYYYMM` (vendor's compact form preserved).
    pub month: String,
    /// 100-base index (e.g. 101.2 = +1.2% YoY).
    pub nation_value: Option<f64>,
    pub nation_yoy: Option<f64>,
    pub nation_mom: Option<f64>,
    pub nation_accum_yoy: Option<f64>,
}

/// One observation in a quarterly GDP series.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GdpObs {
    /// `2025Q4` style.
    pub quarter: String,
    /// Headline GDP in 亿元.
    pub gdp_yi_yuan: Option<f64>,
    pub gdp_yoy: Option<f64>,
    pub primary_yoy: Option<f64>,
    pub secondary_yoy: Option<f64>,
    pub tertiary_yoy: Option<f64>,
}

/// One month of PMI — only the two headline series.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PmiObs {
    pub month: String,
    /// 制造业 PMI (vendor field PMI010000).
    pub manufacturing: Option<f64>,
    /// 非制造业商务活动 PMI (vendor field PMI020100).
    pub non_manufacturing: Option<f64>,
}

/// One month of M0/M1/M2 money-supply observations (亿元).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoneyObs {
    pub month: String,
    pub m0_yi_yuan: Option<f64>,
    pub m1_yi_yuan: Option<f64>,
    pub m2_yi_yuan: Option<f64>,
    pub m0_yoy: Option<f64>,
    pub m1_yoy: Option<f64>,
    pub m2_yoy: Option<f64>,
}

/// One month of social-financing aggregate flow (亿元).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SocialFinancingObs {
    pub month: String,
    pub increment_yi_yuan: Option<f64>,
    pub cumulative_yi_yuan: Option<f64>,
    pub stock_wan_yi_yuan: Option<f64>,
}

/// One LPR observation (in percent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShiborLprObs {
    /// ISO date.
    pub date: String,
    pub lpr_1y: Option<f64>,
    pub lpr_5y: Option<f64>,
}

/// One observation of US Treasury bill rates (in percent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsTbrObs {
    pub date: String,
    /// 4-week bond discount.
    pub w4_bd: Option<f64>,
    /// 13-week bond discount.
    pub w13_bd: Option<f64>,
    /// 52-week bond discount.
    pub w52_bd: Option<f64>,
}

// ── Index snapshot (used by market_overview / industry_landscape) ─────

/// One index daily basic snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexBasic {
    /// `000001.SH` etc.
    pub ts_code: String,
    pub trade_date: String,
    pub total_mv_yuan: Option<f64>,
    pub pe: Option<f64>,
    pub pe_ttm: Option<f64>,
    pub pb: Option<f64>,
    pub turnover_rate: Option<f64>,
}

/// One row of `daily_info`: per-market stats (沪/深 A/B etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketTotals {
    pub ts_code: String,
    pub ts_name: String,
    pub com_count: Option<f64>,
    pub total_mv_yi_yuan: Option<f64>,
    pub amount_yi_yuan: Option<f64>,
    pub pe: Option<f64>,
    pub turnover_rate: Option<f64>,
    pub trade_date: String,
}

/// Push2 realtime snapshot — used by `market_pulse` and `stock_overview`
/// when the trade day is current.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LiveQuote {
    pub symbol: String,
    pub name: Option<String>,
    pub price: Option<f64>,
    pub change: Option<f64>,
    pub change_pct: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    pub volume_shares: Option<f64>,
    pub turnover_yuan: Option<f64>,
    pub turnover_rate: Option<f64>,
    pub timestamp_unix: Option<i64>,
}

/// Push2 realtime capital flow snapshot (per-symbol).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LiveFlow {
    pub symbol: String,
    pub main_net_yuan: Option<f64>,
    pub main_net_pct: Option<f64>,
    pub super_net_yuan: Option<f64>,
    pub large_net_yuan: Option<f64>,
    pub medium_net_yuan: Option<f64>,
    pub small_net_yuan: Option<f64>,
}

// ── OHLCV candle ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Candle {
    /// ISO 8601 (`2026-05-20`).
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Shares.
    pub volume: f64,
    /// Yuan.
    pub turnover: Option<f64>,
}

/// Technical-indicator row from Tushare's `stk_factor`. All values are
/// raw numbers — the agent decides what "oversold" / "breakout" mean.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TechRow {
    pub date: String,
    pub close: Option<f64>,
    pub macd_dif: Option<f64>,
    pub macd_dea: Option<f64>,
    pub macd: Option<f64>,
    pub kdj_k: Option<f64>,
    pub kdj_d: Option<f64>,
    pub kdj_j: Option<f64>,
    pub rsi_6: Option<f64>,
    pub rsi_12: Option<f64>,
    pub rsi_24: Option<f64>,
    pub boll_upper: Option<f64>,
    pub boll_mid: Option<f64>,
    pub boll_lower: Option<f64>,
}

// ── Financial statement rows ──────────────────────────────────────────

/// One reporting-period worth of financial fields. Statement-specific
/// keys live in `metrics` — see `Tushare` adapter for the canonical list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinancialRow {
    /// ISO 8601 period end (e.g. `2024-12-31`).
    pub period_end: String,
    /// Period label, e.g. `2024Q4` / `2024`.
    pub label: String,
    pub metrics: BTreeMap<String, Option<f64>>,
}

// ── Company profile ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompanyProfile {
    pub symbol: String,
    pub name: String,
    pub chairman: Option<String>,
    pub secretary: Option<String>,
    pub industry: Option<String>,
    pub area: Option<String>,
    pub exchange_label: Option<String>,
    pub list_date: Option<String>,
    pub introduction: Option<String>,
    pub main_business: Option<String>,
    pub website: Option<String>,
    pub employees: Option<f64>,
    /// `stock_basic.fullname` — full registered name.
    pub fullname: Option<String>,
}

// ── Top holders ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HolderRow {
    pub rank: u32,
    pub holder_name: String,
    pub holder_type: Option<String>,
    pub shares: Option<f64>,
    pub pct: Option<f64>,
    pub change_qoq_shares: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HolderCountObs {
    /// ISO date of the report period end.
    pub end_date: String,
    pub holder_count: Option<f64>,
}

// ── Business breakdown row ────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BusinessRow {
    /// `product` / `industry` / `region` dimension type, echoed back.
    pub dimension: String,
    pub item: String,
    pub revenue_yuan: Option<f64>,
    pub pct_of_total: Option<f64>,
    pub gross_margin_pct: Option<f64>,
    /// Period the row applies to (ISO 8601).
    pub period_end: String,
}

// ── Announcement / event ──────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnnouncementItem {
    pub date: String,
    pub title: String,
    /// Free-text keyword tag (e.g. `分红配股`, `重大事项`).
    pub category: Option<String>,
    pub url: Option<String>,
    /// Direct PDF URL if known (eastmoney `pdf.dfcfw.com/pdf/H2_*.pdf`).
    pub pdf_url: Option<String>,
}

/// Stock-holder trade / 高管增减持 row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InsiderTradeItem {
    pub ann_date: String,
    pub holder_name: String,
    /// `IN`(增持) / `DE`(减持).
    pub direction: Option<String>,
    pub change_vol_shares: Option<f64>,
    pub change_ratio_pct: Option<f64>,
    pub after_shares: Option<f64>,
    pub after_ratio_pct: Option<f64>,
}

/// Dividend / 分红 event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DividendItem {
    pub end_date: String,
    pub ann_date: String,
    pub process: Option<String>,
    pub stk_div: Option<f64>,
    pub cash_div_pretax: Option<f64>,
    pub ex_date: Option<String>,
    pub pay_date: Option<String>,
}

/// Share unlock / 限售解禁 event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShareUnlockItem {
    pub ann_date: String,
    pub float_date: String,
    pub float_share: Option<f64>,
    pub float_ratio: Option<f64>,
    pub holder_name: Option<String>,
    pub share_type: Option<String>,
}

/// Block trade / 大宗交易.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockTradeItem {
    pub trade_date: String,
    pub price: Option<f64>,
    pub vol_wan_shares: Option<f64>,
    pub amount_wan_yuan: Option<f64>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
}

/// Lung-hu-bang / 龙虎榜 row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TopListItem {
    pub trade_date: String,
    pub close: Option<f64>,
    pub pct_change: Option<f64>,
    pub turnover_rate: Option<f64>,
    pub net_amount: Option<f64>,
    pub l_buy: Option<f64>,
    pub l_sell: Option<f64>,
    pub reason: Option<String>,
}

/// Pledge / 质押 status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PledgeStatItem {
    pub end_date: String,
    pub pledge_count: Option<f64>,
    pub unrest_pledge_wan: Option<f64>,
    pub rest_pledge_wan: Option<f64>,
    pub total_share_wan: Option<f64>,
    pub pledge_ratio_pct: Option<f64>,
}

/// Repurchase / 回购 event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepurchaseItem {
    pub ann_date: String,
    pub end_date: Option<String>,
    pub proc: Option<String>,
    pub vol_share: Option<f64>,
    pub amount_yuan: Option<f64>,
    pub high_limit: Option<f64>,
    pub low_limit: Option<f64>,
}

/// Institution visit / 机构调研 record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstitutionVisitItem {
    pub surv_date: String,
    pub receivers: Option<String>,
    pub place: Option<String>,
    pub mode: Option<String>,
    pub visiting_org: Option<String>,
    pub org_type: Option<String>,
    /// 公司接待人(董秘 / IR 等).
    pub host: Option<String>,
}

// ── Sell-side research (research_sentiment) ───────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RatingMix {
    pub buy: usize,
    pub overweight: usize,
    pub hold: usize,
    pub underweight: usize,
    pub sell: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct YearForecast {
    /// `2025` / `2026`.
    pub year: String,
    pub eps_mean: Option<f64>,
    pub eps_high: Option<f64>,
    pub eps_low: Option<f64>,
    pub net_profit_mean_yi_yuan: Option<f64>,
    pub net_profit_high_yi_yuan: Option<f64>,
    pub net_profit_low_yi_yuan: Option<f64>,
    /// Distinct contributing report count.
    pub sample_size: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResearchReportItem {
    pub date: String,
    pub title: String,
    pub org_name: Option<String>,
    pub author: Option<String>,
    pub rating: Option<String>,
    pub target_price: Option<f64>,
    /// Direct PDF URL (eastmoney H3_*) or sourced URL (tushare's `url`).
    pub pdf_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrokerMonthlyPick {
    pub month: String,
    pub broker: String,
    pub ts_code: String,
    pub name: String,
}

// ── Industry leaders / valuation rows ─────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndustryLeaderRow {
    pub ts_code: String,
    pub name: String,
    pub total_mv_yi_yuan: Option<f64>,
    pub pe_ttm: Option<f64>,
    pub pb: Option<f64>,
    pub dv_ttm: Option<f64>,
    pub turnover_rate: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndustryFlowRow {
    pub ts_code: String,
    pub name: String,
    pub pct_change: Option<f64>,
    pub net_amount_yuan: Option<f64>,
    pub net_amount_rate: Option<f64>,
    pub rank: Option<u32>,
    pub lead_stock: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LimitCapt {
    pub ts_code: String,
    pub name: String,
    pub days: Option<u32>,
    pub up_stat: Option<String>,
    pub cons_nums: Option<String>,
    pub up_nums: Option<u32>,
    pub pct_change: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LimitListRow {
    pub trade_date: String,
    pub ts_code: String,
    pub name: String,
    pub close: Option<f64>,
    pub pct_chg: Option<f64>,
    pub turnover_ratio: Option<f64>,
    /// `U` = up-limit, `D` = down-limit, `Z` = once-limit / 炸板.
    pub limit: Option<String>,
}

// ── Policy / calendar (macro_indicators policy + calendar focus) ──────

/// One official-policy entry from the State Council policy library.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyItem {
    /// Publication date (ISO 8601, e.g. `2026-05-22`). Source field may
    /// carry a full datetime — the adapter trims to the date.
    pub publish_date: String,
    /// Title / headline.
    pub title: String,
    /// Issuing organ (e.g. 国务院, 财政部).
    pub publish_org: Option<String>,
    /// Vendor policy id / document number (e.g. 国发〔2026〕11号). Used
    /// for de-duplication and as a stable reference.
    pub policy_id: Option<String>,
    /// Category path (e.g. `综合政务\其他`). Echoed verbatim — the agent
    /// decides whether it's relevant.
    pub category: Option<String>,
}

/// One upcoming macro-economic data release on the official calendar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacroEventItem {
    /// Scheduled publication date (ISO 8601). The upstream field is the
    /// expected release date, not the data period it covers.
    pub publish_date: String,
    /// Indicator title (e.g. `居民消费价格指数月度报告`).
    pub title: String,
    /// Issuing organ (e.g. `国家统计局`, `中国人民银行`).
    pub issuing_org: Option<String>,
    /// Data period the release covers (`YYYYMM`, e.g. `202604`).
    pub period: Option<String>,
    /// The tushare `api_name` that holds the actual numbers once the
    /// release is published; `待上线` if no API maps yet. The agent can
    /// use this as a follow-up hint without us having to inline it.
    pub data_api: Option<String>,
}

// ── Concepts (industry_landscape concepts focus) ──────────────────────

/// One entry from the EastMoney concept-board universe (`dc_concept`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptItem {
    /// EastMoney theme code (e.g. `000008.DC`).
    pub theme_code: String,
    /// Concept board name (e.g. `雄安新区`).
    pub name: String,
    /// Last refresh date of the row (ISO 8601).
    pub trade_date: String,
    /// Daily change percent for the board.
    pub pct_change: Option<f64>,
    /// Vendor's daily heat score (positional — higher is hotter).
    pub hot: Option<f64>,
    /// Lead stock for the board.
    pub lead_stock: Option<String>,
    /// Lead stock ts_code (`603098.SH`).
    pub lead_stock_code: Option<String>,
    /// Lead stock daily change percent.
    pub lead_stock_pct: Option<f64>,
    /// Net main-force capital change for the board (yuan, may be negative).
    pub main_change: Option<f64>,
    /// Number of constituent stocks currently at the daily up-limit.
    pub z_t_num: Option<f64>,
}

/// One constituent of a concept board (`dc_concept_cons`).
///
/// Held alongside `dc_concept_cons` (see tushare.rs) as `dead_code` for
/// now — kept on the surface for the next "drill into a concept board"
/// caller.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConceptConstituent {
    pub ts_code: String,
    pub name: String,
    /// SW industry of the constituent (used to filter relevance).
    pub industry: Option<String>,
    /// Free-text reason the stock is in the concept (often very long —
    /// the tool trims before surfacing to model output).
    pub reason: Option<String>,
}

/// `hsgt` total-flow snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HsgtFlow {
    pub trade_date: String,
    /// Total northbound net flow (万元).
    pub north_money_wan: Option<f64>,
    pub south_money_wan: Option<f64>,
    pub hgt_wan: Option<f64>,
    pub sgt_wan: Option<f64>,
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
        assert_eq!(Symbol::parse("600519").unwrap().exchange, "SH");
        assert_eq!(Symbol::parse("000001").unwrap().exchange, "SZ");
        assert_eq!(Symbol::parse("300750").unwrap().exchange, "SZ");
        assert_eq!(Symbol::parse("688981").unwrap().exchange, "SH");
        assert_eq!(Symbol::parse("605378").unwrap().exchange, "SH");
    }

    #[test]
    fn rejects_garbage() {
        assert!(Symbol::parse("").is_err());
        assert!(Symbol::parse("foo").is_err());
        assert!(Symbol::parse("123").is_err());
        assert!(Symbol::parse("999999").is_err());
        assert!(Symbol::parse("600519.XX").is_err());
    }

    /// Regression: tokio-rt-worker panic. An industry name like
    /// `化学制药` (12 bytes, 4 chars) used to reach `split_at(2)` via
    /// the `s.len() >= 8` guard and crash inside `化`. Now the ASCII
    /// guard short-circuits before any byte indexing.
    #[test]
    fn rejects_cjk_industry_name_without_panic() {
        assert!(Symbol::parse("化学制药").is_err());
        assert!(Symbol::parse("恒瑞医药").is_err());
        assert!(Symbol::parse("白酒Ⅱ").is_err());
        assert!(Symbol::parse("  化学制药  ").is_err());
        // A long Chinese string that comfortably exceeds the 8-byte
        // gate — would have hit the buggy `split_at(2)` branch first.
        assert!(Symbol::parse("化学制药行业龙头").is_err());
    }

    #[test]
    fn vendor_error_display_includes_vendor_tag() {
        let e = VendorError::recoverable("tushare", "no token");
        let s = format!("{e}");
        assert!(s.contains("tushare"));
        assert!(s.contains("no token"));
    }

    #[test]
    fn vendor_error_recoverable_flag_drives_fallback() {
        assert!(VendorError::recoverable("v", "x").recoverable);
        assert!(!VendorError::fatal("v", "x").recoverable);
    }
}
