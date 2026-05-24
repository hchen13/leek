//! Upstream data vendor integration layer (M3).
//!
//! Tools in `agent::tools::market_*` are vendor-neutral by name and by
//! description — see ARCHITECTURE §12.1. The actual upstream identity
//! (Tushare, Sina, EastMoney) only lives in:
//!
//! - module file names under `vendors/` (`tushare.rs`, `sina.rs`, …)
//! - struct field comments
//! - env var names (`LEEK_TUSHARE_TOKEN`)
//! - `tracing` log fields (`vendor = "tushare"`)
//!
//! It MUST NOT appear in `ToolSpec.name`, `ToolSpec.description`, JSON
//! schema property names, `model_output`, or `display_payload`. The grep
//! check at `tests/m3-vendor-neutrality.rs` is the guardrail.
//!
//! ## Architecture
//!
//! Each tool defines an output struct (`MarketQuote`, `Candle`, …) and
//! one trait per data shape (`VendorQuote`, `VendorCandle`, …). Every
//! vendor implements zero or more traits — Tushare implements all five
//! (everything), Sina only `VendorQuote`, EastMoney only `VendorCandle`.
//!
//! The dispatch path walks a fallback chain (Tushare → Sina → EastMoney
//! as configured per tool), calling `fetch_*` on each and returning the
//! first non-error. A vendor returning a structured error (e.g. quota
//! exhausted) does not short-circuit — we fall through to the next.
//!
//! The shape that goes back to the model (model_output / display_payload)
//! is the same regardless of which vendor served it; only `data_source`
//! (a tag like "primary", "fallback-1", "stale") and a hidden debug
//! field track the actual vendor for observability.

pub mod eastmoney;
pub mod sina;
pub mod tushare;
pub mod types;
pub mod vendor_trait;

pub use eastmoney::EastmoneyClient;
pub use sina::SinaClient;
pub use tushare::TushareClient;
pub use types::*;
pub use vendor_trait::{
    VendorCandle, VendorCapitalFlow, VendorCompanyInfo, VendorFinancial, VendorQuote,
};
#[allow(unused_imports)]
pub use vendor_trait::VendorError;

use std::sync::Arc;

/// Holds the full vendor lineup — built once at startup, shared in
/// `AppState`. Each `Arc` is cheap to clone per tool dispatch.
#[derive(Clone)]
pub struct VendorRegistry {
    pub tushare: Arc<TushareClient>,
    pub sina: Arc<SinaClient>,
    pub eastmoney: Arc<EastmoneyClient>,
}

impl VendorRegistry {
    /// Build the default registry: read `LEEK_TUSHARE_TOKEN` env first,
    /// fall back to `Config::tushare_token` (added in M3). Sina + EastMoney
    /// are token-free and always available.
    pub fn from_env_and_config(
        http: reqwest::Client,
        config_token: Option<String>,
    ) -> Self {
        let tushare_token = std::env::var("LEEK_TUSHARE_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
            .or(config_token)
            .filter(|s| !s.is_empty());
        if tushare_token.is_some() {
            tracing::info!(vendor = "tushare", "tushare token configured");
        } else {
            tracing::info!(
                vendor = "tushare",
                "no tushare token; tools will use fallback vendors only"
            );
        }
        Self {
            tushare: Arc::new(TushareClient::new(http.clone(), tushare_token)),
            sina: Arc::new(SinaClient::new(http.clone())),
            eastmoney: Arc::new(EastmoneyClient::new(http)),
        }
    }

    /// Convenience for tests that don't need real HTTP — every vendor
    /// gets a default client; calls will fail without a network but the
    /// registry composes cleanly.
    #[cfg(test)]
    pub fn for_test() -> Self {
        let http = reqwest::Client::new();
        Self {
            tushare: Arc::new(TushareClient::new(http.clone(), None)),
            sina: Arc::new(SinaClient::new(http.clone())),
            eastmoney: Arc::new(EastmoneyClient::new(http)),
        }
    }
}
