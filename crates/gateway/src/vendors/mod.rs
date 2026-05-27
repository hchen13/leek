//! Upstream data vendor integration layer (M4.1.1 facts-only).
//!
//! Tools in `agent::tools::*` are vendor-neutral by name and by
//! description — see ARCHITECTURE §12.1. The actual upstream identity
//! (Tushare, EastMoney) only lives in:
//!
//! - module file names under `vendors/` (`tushare.rs`, `eastmoney.rs`)
//! - struct field comments + log fields
//! - env var names (`LEEK_TUSHARE_TOKEN`)
//! - `tracing` log fields (`vendor = "tushare"`)
//!
//! It MUST NOT appear in `ToolSpec.name`, `ToolSpec.description`, JSON
//! schema property names, `model_output`, or `display_payload`. The grep
//! check at `tests/m3_vendor_neutrality.rs` is the guardrail.
//!
//! ## Architecture (post M4.1.1)
//!
//! M4.1.1 dropped the per-shape `Vendor*` trait pattern. The new tools
//! fan-out internally — each tool calls 2-5 concrete vendor methods
//! concurrently and stitches a distilled result. We no longer need
//! "any vendor implementing shape X is interchangeable" because tools
//! pick endpoints deliberately and surface gaps as `empty_dimensions`,
//! not vendor swaps.
//!
//! So `vendors/` now exposes `TushareClient` and `EastmoneyClient`
//! directly as small async wrappers around their HTTP surfaces. Each
//! method returns a typed shape from `types.rs` and a `VendorError` on
//! failure. Tools `try_join!` several of these at once.

pub mod eastmoney;
pub mod tushare;
pub mod types;

pub use eastmoney::EastmoneyClient;
pub use tushare::TushareClient;
pub use types::*;

use std::sync::Arc;

/// Holds the vendor lineup — built once at startup, shared in
/// `AppState`. Each `Arc` is cheap to clone per tool dispatch.
#[derive(Clone)]
pub struct VendorRegistry {
    pub tushare: Arc<TushareClient>,
    pub eastmoney: Arc<EastmoneyClient>,
}

impl VendorRegistry {
    /// Build the default registry. Tushare token resolution order:
    /// `LEEK_TUSHARE_TOKEN` env > `Config::tushare_token` > none. With
    /// no token Tushare methods all return a recoverable error tagged
    /// "no token configured" — tools surface that as an `empty_dimensions`
    /// entry, never silently fabricate.
    pub fn from_env_and_config(http: reqwest::Client, config_token: Option<String>) -> Self {
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
                "no tushare token; A-share tools will surface empty dimensions"
            );
        }
        Self {
            tushare: Arc::new(TushareClient::new(http.clone(), tushare_token)),
            eastmoney: Arc::new(EastmoneyClient::new(http)),
        }
    }

    /// Convenience for tests that don't need real HTTP.
    #[cfg(test)]
    pub fn for_test() -> Self {
        let http = reqwest::Client::new();
        Self {
            tushare: Arc::new(TushareClient::new(http.clone(), None)),
            eastmoney: Arc::new(EastmoneyClient::new(http)),
        }
    }
}
