//! EastMoney push2his — A-share candlestick fallback (M3).
//!
//! Endpoint: `https://push2his.eastmoney.com/api/qt/stock/kline/get`. Required
//! query params:
//!
//! | param  | meaning                                                |
//! | ------ | ------------------------------------------------------ |
//! | secid  | `1.<code>` (SH) / `0.<code>` (SZ)                      |
//! | klt    | 101 = day, 102 = week, 103 = month                     |
//! | fqt    | 1 = forward-adjust (we want this for split alignment)  |
//! | end    | `20500101` (a sentinel "max")                          |
//! | lmt    | row count                                              |
//! | fields1| `f1,f2,f3,f4,f5,f6` (metadata)                         |
//! | fields2| `f51,f52,f53,f54,f55,f56,f57,f58` (date,o,c,h,l,v,a)   |
//!
//! Response shape:
//!
//! ```json
//! { "data": { "klines": ["2026-05-20,1810,1825,1830,1805,2456789,4485000000,...", ...] } }
//! ```
//!
//! Comma-separated string per row. EastMoney only implements
//! `VendorCandle`. Other shapes fall through to the next vendor.

use serde::Deserialize;

use super::types::{Candle, Symbol};
use super::vendor_trait::{BoxFuture, VendorCandle, VendorError};

const VENDOR: &str = "eastmoney";
const EASTMONEY_KLINE_URL: &str = "https://push2his.eastmoney.com/api/qt/stock/kline/get";
const TIMEOUT_SECS: u64 = 10;

pub struct EastmoneyClient {
    http: reqwest::Client,
}

impl EastmoneyClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

#[derive(Debug, Deserialize)]
struct KlineEnvelope {
    data: Option<KlineData>,
}

#[derive(Debug, Deserialize)]
struct KlineData {
    klines: Option<Vec<String>>,
}

impl VendorCandle for EastmoneyClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }

    fn fetch_candles<'a>(
        &'a self,
        symbol: &'a Symbol,
        period: &'a str,
        count: usize,
    ) -> BoxFuture<'a, Result<Vec<Candle>, VendorError>> {
        Box::pin(async move {
            let klt = match period {
                "1d" => "101",
                "1w" => "102",
                "1mo" => "103",
                other => {
                    return Err(VendorError::fatal(
                        VENDOR,
                        format!("unsupported period '{other}' (try 1d/1w/1mo)"),
                    ));
                }
            };
            let secid = symbol.to_eastmoney();
            let resp = self
                .http
                .get(EASTMONEY_KLINE_URL)
                .query(&[
                    ("secid", secid.as_str()),
                    ("klt", klt),
                    ("fqt", "1"), // forward-adjusted prices
                    ("end", "20500101"),
                    ("lmt", &count.to_string()),
                    ("fields1", "f1,f2,f3,f4,f5,f6"),
                    ("fields2", "f51,f52,f53,f54,f55,f56,f57,f58"),
                ])
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| VendorError::recoverable(VENDOR, format!("HTTP failed: {e}")))?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("HTTP {}", resp.status()),
                ));
            }
            let env: KlineEnvelope = resp.json().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("JSON parse failed: {e}"))
            })?;
            let klines = env
                .data
                .and_then(|d| d.klines)
                .ok_or_else(|| VendorError::recoverable(VENDOR, "empty klines envelope"))?;
            let candles: Vec<Candle> = klines
                .iter()
                .filter_map(|s| parse_kline_row(s))
                .collect();
            if candles.is_empty() {
                return Err(VendorError::recoverable(VENDOR, "no candles in response"));
            }
            Ok(candles)
        })
    }
}

/// Parse one EastMoney kline row: `date,open,close,high,low,volume,turnover,amplitude%[,…]`.
/// Public-in-crate for unit tests.
pub(crate) fn parse_kline_row(raw: &str) -> Option<Candle> {
    let fields: Vec<&str> = raw.split(',').collect();
    if fields.len() < 7 {
        return None;
    }
    let date = fields[0].to_string();
    let open: f64 = fields[1].parse().ok()?;
    let close: f64 = fields[2].parse().ok()?;
    let high: f64 = fields[3].parse().ok()?;
    let low: f64 = fields[4].parse().ok()?;
    let volume: f64 = fields[5].parse().unwrap_or(0.0) * 100.0; // 手 → 股
    let turnover: f64 = fields[6].parse().unwrap_or(0.0); // already 元
    Some(Candle {
        date,
        open,
        high,
        low,
        close,
        volume,
        turnover: Some(turnover),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_kline_row() {
        // Real EastMoney row format.
        let row = "2026-05-20,1810.00,1825.30,1830.00,1805.10,24567,4485000000,1.38";
        let c = parse_kline_row(row).unwrap();
        assert_eq!(c.date, "2026-05-20");
        assert!((c.open - 1810.00).abs() < 1e-6);
        assert!((c.close - 1825.30).abs() < 1e-6);
        assert!((c.high - 1830.00).abs() < 1e-6);
        assert!((c.low - 1805.10).abs() < 1e-6);
        // 24567 手 → 2,456,700 股
        assert!((c.volume - 2_456_700.0).abs() < 1.0);
        assert!((c.turnover.unwrap() - 4_485_000_000.0).abs() < 1.0);
    }

    #[test]
    fn truncated_row_returns_none() {
        assert!(parse_kline_row("only,three,fields").is_none());
        assert!(parse_kline_row("").is_none());
    }

    #[test]
    fn malformed_number_returns_none() {
        let row = "2026-05-20,notnum,1825.30,1830.00,1805.10,24567,4485000000";
        assert!(parse_kline_row(row).is_none());
    }
}
