//! Sina Finance simple HTTP — A-share quote fallback (M3).
//!
//! Endpoint: `https://hq.sinajs.cn/list=sh600519`. Response:
//!
//! ```text
//! var hq_str_sh600519="贵州茅台,1820.000,1810.000,...";
//! ```
//!
//! No token. No rate limit beyond polite-use. Fields (positional):
//! 0: 股票名称 / 1: 今日开盘 / 2: 昨日收盘 / 3: 当前价 / 4: 今日最高 /
//! 5: 今日最低 / 6: 买一报价 / 7: 卖一报价 / 8: 成交量(股) / 9: 成交额(元) /
//! 30: 日期 / 31: 时间.
//!
//! Sina only implements `VendorQuote`. Other shapes fall through to the
//! next vendor in the chain.

use super::types::{MarketQuote, Symbol};
use super::vendor_trait::{BoxFuture, VendorError, VendorQuote};

const VENDOR: &str = "sina";
const SINA_URL_PREFIX: &str = "https://hq.sinajs.cn/list=";
const TIMEOUT_SECS: u64 = 8;

/// Sina HTTP client — stateless.
pub struct SinaClient {
    http: reqwest::Client,
}

impl SinaClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }
}

impl VendorQuote for SinaClient {
    fn vendor_name(&self) -> &'static str {
        VENDOR
    }

    fn fetch_quote<'a>(
        &'a self,
        symbol: &'a Symbol,
    ) -> BoxFuture<'a, Result<MarketQuote, VendorError>> {
        Box::pin(async move {
            let url = format!("{SINA_URL_PREFIX}{}", symbol.to_prefixed());
            // Sina requires a non-empty Referer or it returns 403.
            let resp = self
                .http
                .get(&url)
                .header("Referer", "https://finance.sina.com.cn")
                .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
                .send()
                .await
                .map_err(|e| {
                    VendorError::recoverable(VENDOR, format!("HTTP failed: {e}"))
                })?;
            if !resp.status().is_success() {
                return Err(VendorError::recoverable(
                    VENDOR,
                    format!("HTTP {}", resp.status()),
                ));
            }
            // Sina returns GBK-encoded text — use bytes + best-effort
            // UTF-8 fallback. Most field values are ASCII (prices,
            // dates); only the name is Chinese and we tolerate a garbled
            // name rather than dragging in an encoding crate just for
            // this fallback path. (Tushare is the primary; users that
            // care about Chinese name labels should configure a token.)
            let bytes = resp.bytes().await.map_err(|e| {
                VendorError::recoverable(VENDOR, format!("body read failed: {e}"))
            })?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            parse_quote(&text, symbol)
        })
    }
}

/// Parse one `var hq_str_sh600519="...";` line into a `MarketQuote`.
/// Public-in-crate for unit tests.
pub(crate) fn parse_quote(raw: &str, symbol: &Symbol) -> Result<MarketQuote, VendorError> {
    // Find the inner double-quoted payload.
    let start = raw
        .find('"')
        .ok_or_else(|| VendorError::recoverable(VENDOR, "no opening quote in body"))?;
    let end = raw
        .rfind('"')
        .ok_or_else(|| VendorError::recoverable(VENDOR, "no closing quote in body"))?;
    if end <= start + 1 {
        return Err(VendorError::recoverable(
            VENDOR,
            "empty quote body (symbol likely invalid or delisted)",
        ));
    }
    let inner = &raw[start + 1..end];
    let fields: Vec<&str> = inner.split(',').collect();
    if fields.len() < 32 {
        return Err(VendorError::recoverable(
            VENDOR,
            format!("expected ≥32 fields, got {}", fields.len()),
        ));
    }
    let name = if fields[0].is_empty() {
        None
    } else {
        Some(fields[0].to_string())
    };
    let open: f64 = fields[1].parse().unwrap_or(0.0);
    let prev_close: f64 = fields[2].parse().unwrap_or(0.0);
    let price: f64 = fields[3].parse().map_err(|_| {
        VendorError::recoverable(VENDOR, format!("invalid price '{}'", fields[3]))
    })?;
    let high: f64 = fields[4].parse().unwrap_or(0.0);
    let low: f64 = fields[5].parse().unwrap_or(0.0);
    let volume: f64 = fields[8].parse().unwrap_or(0.0);
    let turnover: f64 = fields[9].parse().unwrap_or(0.0);
    let date = fields[30].to_string();
    let time = fields[31].to_string();
    let change = price - prev_close;
    let change_pct = if prev_close > 0.0 {
        (change / prev_close) * 100.0
    } else {
        0.0
    };
    // Mark "stale" if the price is 0 — happens on weekends / pre-open.
    let freshness = if price == 0.0 { "stale" } else { "realtime" };
    Ok(MarketQuote {
        symbol: symbol.to_dotted(),
        name,
        price,
        change,
        change_pct,
        volume: Some(volume),
        turnover: Some(turnover),
        open: Some(open),
        high: Some(high),
        low: Some(low),
        prev_close: Some(prev_close),
        timestamp: if date.is_empty() {
            "".into()
        } else if time.is_empty() {
            date
        } else {
            format!("{date}T{time}+08:00")
        },
        data_freshness: freshness.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Sina response shape (synthetic but field-accurate).
    fn fixture(name: &str) -> String {
        // 32 fields per Sina spec, comma-separated.
        let fields = [
            name,         // 0 name
            "1810.00",    // 1 open
            "1800.50",    // 2 prev_close
            "1825.30",    // 3 current price
            "1830.00",    // 4 high
            "1805.10",    // 5 low
            "1825.30",    // 6 bid1
            "1825.40",    // 7 ask1
            "2456789",    // 8 volume
            "4485000000", // 9 turnover
            "100",        // 10 bid1_size
            "1825.30",    // 11 bid1_price (dup of 6)
            "200",        // 12 bid2_size
            "1825.20",    // 13 bid2_price
            "0",
            "0",
            "0",
            "0",
            "0",
            "0", // 14-19 bids
            "100",
            "1825.40",
            "200",
            "1825.50",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0", // 20-29 asks
            "2026-05-20", // 30 date
            "15:00:00",   // 31 time
        ];
        format!("var hq_str_sh600519=\"{}\";\n", fields.join(","))
    }

    #[test]
    fn parses_normal_quote() {
        let s = Symbol::parse("600519.SH").unwrap();
        let q = parse_quote(&fixture("贵州茅台"), &s).unwrap();
        assert_eq!(q.symbol, "600519.SH");
        assert_eq!(q.name.as_deref(), Some("贵州茅台"));
        assert!((q.price - 1825.30).abs() < 1e-6);
        assert!((q.prev_close.unwrap() - 1800.50).abs() < 1e-6);
        // Sanity: change = price - prev_close, sign consistent
        assert!((q.change - 24.80).abs() < 1e-6);
        assert!(q.change_pct > 0.0);
        assert_eq!(q.timestamp, "2026-05-20T15:00:00+08:00");
        assert_eq!(q.data_freshness, "realtime");
    }

    #[test]
    fn empty_payload_is_recoverable_error() {
        // Sina returns `var hq_str_xxx="";` for invalid / delisted symbols.
        let raw = "var hq_str_sh999999=\"\";";
        let q = parse_quote(raw, &Symbol { code: "999999".into(), exchange: "SH".into() });
        assert!(q.is_err(), "empty payload should fail to parse, got {q:?}");
    }

    #[test]
    fn missing_quotes_in_body_is_recoverable_error() {
        let q = parse_quote(
            "no quotes anywhere here",
            &Symbol { code: "600519".into(), exchange: "SH".into() },
        );
        assert!(q.is_err());
    }

    #[test]
    fn truncated_body_short_of_32_fields_errors() {
        let raw = "var hq_str_sh600519=\"a,b,c,d\";";
        let q = parse_quote(
            raw,
            &Symbol { code: "600519".into(), exchange: "SH".into() },
        );
        assert!(q.is_err());
        assert!(q.unwrap_err().message.contains("32 fields"));
    }
}
