//! Vendor trait set — one per tool data shape.
//!
//! Each vendor implements zero or more of these. The dispatch path in
//! the tool modules walks a fallback chain (per-tool ordering) and
//! returns the first non-error response.
//!
//! Trait names use generic shape vocabulary (`VendorQuote`, …), NOT a
//! vendor name. The chosen vendor only shows up via `tracing` log
//! fields, not in any public surface.
//!
//! We use the hand-rolled `Pin<Box<dyn Future>>` pattern rather than
//! `async_trait` — the trait surface is small (five traits, one method
//! each) and adding a workspace dep just for syntactic sugar isn't
//! worth it.

use std::future::Future;
use std::pin::Pin;

use super::types::{
    AnalystConsensus, AnnouncementList, BusinessBreakdown, CapitalFlow, Candle, CompanyInfo,
    ConceptList, FinancialReport, IndustryPeers, MarketQuote, Symbol, TopHolders,
};

/// Boxed future for trait return types — keeps `dyn VendorX` object-safe
/// without pulling in `async_trait`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One vendor's reason for not serving a request. The dispatch path
/// reads the `recoverable` flag — `true` means "try the next vendor",
/// `false` means "stop and surface this".
#[derive(Debug, Clone)]
pub struct VendorError {
    pub vendor: &'static str,
    pub message: String,
    pub recoverable: bool,
}

impl VendorError {
    /// A recoverable error — the dispatch path moves on to the next
    /// vendor. Use for missing tokens, HTTP errors, quota exhaustion.
    pub fn recoverable(vendor: &'static str, message: impl Into<String>) -> Self {
        Self {
            vendor,
            message: message.into(),
            recoverable: true,
        }
    }

    /// A hard error — input was malformed in a way no other vendor can
    /// fix (e.g. invalid symbol). Surfaces immediately.
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

/// Snapshot quote.
pub trait VendorQuote: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_quote<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<MarketQuote, VendorError>>;
}

/// Historical OHLCV bars.
pub trait VendorCandle: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_candles<'a>(
        &'a self,
        symbol: &'a Symbol,
        period: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<Vec<Candle>, VendorError>>;
}

/// Income / balance / cashflow / ratio fundamentals.
pub trait VendorFinancial: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_financials<'a>(
        &'a self,
        symbol: &'a Symbol,
        statement: &'a str,
        period: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<FinancialReport, VendorError>>;
}

/// Company profile + latest indicators.
pub trait VendorCompanyInfo: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_company_info<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<CompanyInfo, VendorError>>;
}

/// Capital flow over a window. The trait does NOT mandate northbound
/// support — implementers populate `north_flow_available` per vendor.
pub trait VendorCapitalFlow: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_capital_flow<'a>(
        &'a self,
        symbol: &'a Symbol,
        period: &'a str,
    ) -> BoxFuture<'a, Result<CapitalFlow, VendorError>>;
}

// ── M4.1 — supplementary research traits ─────────────────────────────

/// Same-industry comparison set. Vendors are free to choose the peer
/// list internally (industry classification is implementation-detail);
/// `IndustryPeers.industry` echoes back the bucket used.
pub trait VendorIndustryPeers: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_industry_peers<'a>(
        &'a self,
        symbol: &'a Symbol,
        dimension: &'a str,
    ) -> BoxFuture<'a, Result<IndustryPeers, VendorError>>;
}

/// Main-business revenue split for one reporting period.
pub trait VendorBusinessBreakdown: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_business_breakdown<'a>(
        &'a self,
        symbol: &'a Symbol,
        period: &'a str,
        dim: &'a str,
    ) -> BoxFuture<'a, Result<BusinessBreakdown, VendorError>>;
}

/// Recent corporate announcements for one symbol.
pub trait VendorAnnouncements: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_announcements<'a>(
        &'a self,
        symbol: &'a Symbol,
        days: usize,
        category: Option<&'a str>,
    ) -> BoxFuture<'a, Result<AnnouncementList, VendorError>>;
}

/// Sell-side analyst consensus + rating mix.
pub trait VendorConsensus: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_consensus<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<AnalystConsensus, VendorError>>;
}

/// Top-10 shareholders (either total or float).
pub trait VendorTopHolders: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_top_holders<'a>(
        &'a self,
        symbol: &'a Symbol,
        kind: &'a str,
    ) -> BoxFuture<'a, Result<TopHolders, VendorError>>;
}

/// Concept / theme tags for one symbol.
pub trait VendorConcepts: Send + Sync {
    fn vendor_name(&self) -> &'static str;
    fn fetch_concepts<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<ConceptList, VendorError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

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
