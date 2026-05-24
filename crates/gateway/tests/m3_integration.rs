//! M3 integration smoke — fetch one real row per tool via the live
//! upstream vendors. Network-dependent: skipped by default,
//! `#[ignore]`. Run manually with:
//!
//! ```bash
//! LEEK_TUSHARE_TOKEN=... cargo test --test m3_integration -- --ignored
//! ```
//!
//! Like `m3_vendor_neutrality`, this test cannot import the gateway's
//! modules (the crate is `bin` only). Instead it issues raw HTTP
//! requests that mirror what each vendor module would issue and
//! asserts the response shape we depend on. If an upstream API ever
//! changes shape this catches it before a user hits a parse error in
//! production.

use std::time::Duration;

const TUSHARE_URL: &str = "http://api.tushare.pro";
const SINA_URL: &str = "https://hq.sinajs.cn/list=sh600519";
const EASTMONEY_URL: &str =
    "https://push2his.eastmoney.com/api/qt/stock/kline/get";

fn token() -> Option<String> {
    std::env::var("LEEK_TUSHARE_TOKEN").ok().filter(|s| !s.is_empty())
}

#[tokio::test]
#[ignore = "network + requires LEEK_TUSHARE_TOKEN"]
async fn primary_quote_endpoint_responds() {
    let Some(tok) = token() else {
        // Soft-skip: the test is opt-in via `--ignored` already, but
        // even within `--ignored` we want a clean pass when the token
        // is missing rather than a panic — otherwise CI without the
        // secret would always fail this run-mode.
        eprintln!("skip: LEEK_TUSHARE_TOKEN not set");
        return;
    };
    let http = reqwest::Client::new();
    let body = serde_json::json!({
        "api_name": "daily",
        "token": tok,
        "params": { "ts_code": "600519.SH" },
        "fields": "ts_code,trade_date,close",
    });
    let resp = http
        .post(TUSHARE_URL)
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("send");
    assert!(resp.status().is_success());
    let j: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(j["code"], 0);
    assert!(j["data"]["fields"].is_array());
    assert!(j["data"]["items"].is_array());
}

#[tokio::test]
#[ignore = "network"]
async fn fallback_quote_endpoint_responds() {
    let http = reqwest::Client::new();
    let resp = http
        .get(SINA_URL)
        .header("Referer", "https://finance.sina.com.cn")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .expect("send");
    assert!(resp.status().is_success());
    let body = resp.bytes().await.expect("body");
    let text = String::from_utf8_lossy(&body);
    assert!(text.starts_with("var hq_str_sh600519"));
}

#[tokio::test]
#[ignore = "network"]
async fn fallback_candle_endpoint_responds() {
    let http = reqwest::Client::new();
    let resp = http
        .get(EASTMONEY_URL)
        .query(&[
            ("secid", "1.600519"),
            ("klt", "101"),
            ("fqt", "1"),
            ("end", "20500101"),
            ("lmt", "10"),
            ("fields1", "f1,f2,f3,f4,f5,f6"),
            ("fields2", "f51,f52,f53,f54,f55,f56,f57,f58"),
        ])
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("send");
    assert!(resp.status().is_success());
    let j: serde_json::Value = resp.json().await.expect("json");
    let klines = j["data"]["klines"].as_array().expect("klines array");
    assert!(!klines.is_empty(), "expected at least one kline row");
    let row = klines[0].as_str().expect("kline row is a string");
    let fields: Vec<&str> = row.split(',').collect();
    assert!(fields.len() >= 7, "kline row has ≥7 fields: {row}");
}
