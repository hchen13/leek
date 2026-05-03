use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::DateTime;
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_candlesticks";
const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";
const TUSHARE_FIELDS: &str = "ts_code,trade_date,open,high,low,close,vol,amount,pct_chg";
const YAHOO_BASE: &str = "https://query1.finance.yahoo.com/v8/finance/chart";
const BINANCE_BASE: &str = "https://api.binance.com/api/v3/klines";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 200;

pub struct GetCandlesticksTool {
    http: Client,
}

impl GetCandlesticksTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
            )
            .build()?;
        Ok(Self { http })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Market {
    AShare,
    UsStock,
    HkStock,
    Crypto,
}

#[derive(Deserialize, Default, Clone, Copy)]
enum Interval {
    #[serde(rename = "1d")]
    #[default]
    Day,
    #[serde(rename = "1w")]
    Week,
    #[serde(rename = "1mo")]
    Month,
}

impl Interval {
    fn tushare_freq(&self) -> &str {
        match self {
            Interval::Day => "D",
            Interval::Week => "W",
            Interval::Month => "M",
        }
    }

    fn yahoo_interval(&self) -> &str {
        match self {
            Interval::Day => "1d",
            Interval::Week => "1wk",
            Interval::Month => "1mo",
        }
    }

    fn binance_interval(&self) -> &str {
        match self {
            Interval::Day => "1d",
            Interval::Week => "1w",
            Interval::Month => "1M",
        }
    }

    fn label(&self) -> &str {
        match self {
            Interval::Day => "Daily",
            Interval::Week => "Weekly",
            Interval::Month => "Monthly",
        }
    }

    fn label_cn(&self) -> &str {
        match self {
            Interval::Day => "日线",
            Interval::Week => "周线",
            Interval::Month => "月线",
        }
    }
}

#[derive(Deserialize)]
struct Args {
    ticker: String,
    market: Market,
    #[serde(default)]
    interval: Interval,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    adjusted: Option<bool>,
}

#[async_trait]
impl ToolHandler for GetCandlesticksTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch historical OHLCV (K-line) data for any tradable asset.\n\
                Market is controlled by the `market` parameter — this single tool covers all asset classes.\n\
                - A-share (CN equities): ts_code like \"600519.SH\", \"000001.SZ\", \"300750.SZ\"\n\
                - US/HK stocks: ticker like \"AAPL\", \"TSLA\", \"0700.HK\", \"BABA\"\n\
                - Crypto: Binance symbol like \"BTCUSDT\", \"ETHUSDT\", \"SOLUSDT\"\n\
                Returns a markdown table of OHLCV data sorted oldest→newest.\n\
                Prefer this over tushare_quote for historical analysis spanning multiple days."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ticker": {
                        "type": "string",
                        "description": "Asset symbol in market-appropriate format"
                    },
                    "market": {
                        "type": "string",
                        "enum": ["a_share", "us_stock", "hk_stock", "crypto"]
                    },
                    "interval": {
                        "type": "string",
                        "enum": ["1d", "1w", "1mo"],
                        "description": "Candle interval, default 1d"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of candles (default 30, max 200)"
                    },
                    "adjusted": {
                        "type": "boolean",
                        "description": "Front-adjusted prices (default true, A-share and US only)"
                    }
                },
                "required": ["ticker", "market"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| anyhow!("invalid arguments: {e}"))?;
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT).max(1);
        let adjusted = args.adjusted.unwrap_or(true);

        match args.market {
            Market::AShare => {
                self.fetch_a_share(&args.ticker, args.interval, limit, adjusted, cancel)
                    .await
            }
            Market::UsStock | Market::HkStock => {
                self.fetch_yahoo(&args.ticker, args.interval, limit, adjusted, cancel)
                    .await
            }
            Market::Crypto => {
                self.fetch_binance(&args.ticker, args.interval, limit, cancel)
                    .await
            }
        }
    }
}

impl GetCandlesticksTool {
    async fn fetch_a_share(
        &self,
        ticker: &str,
        interval: Interval,
        limit: usize,
        adjusted: bool,
        cancel: CancellationToken,
    ) -> Result<String> {
        let token = match std::env::var("TUSHARE_TOKEN") {
            Ok(t) => t,
            Err(_) => {
                return Ok(format!(
                    "[get_candlesticks: TUSHARE_TOKEN not configured — \
                     A-share candlesticks unavailable. \
                     Get a free token at https://tushare.pro/register]"
                ))
            }
        };

        let mut params = serde_json::json!({
            "ts_code": ticker,
            "freq": interval.tushare_freq(),
            "limit": limit,
        });
        if adjusted {
            params["adj"] = serde_json::Value::String("qfq".into());
        }

        let payload = serde_json::json!({
            "api_name": "pro_bar",
            "token": token,
            "params": params,
            "fields": TUSHARE_FIELDS,
        });

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before tushare request"),
            r = self.http.post(TUSHARE_ENDPOINT).json(&payload).send() => r?,
        };
        if !resp.status().is_success() {
            bail!("tushare returned HTTP {}", resp.status().as_u16());
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during tushare parse"),
            r = resp.json() => r?,
        };

        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            bail!("tushare error (code={code}): {msg}");
        }

        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("tushare response missing 'data'"))?;
        let fields: Vec<String> = data
            .get("fields")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let items: Vec<&Vec<serde_json::Value>> = data
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|row| row.as_array()).collect())
            .unwrap_or_default();

        if items.is_empty() {
            return Ok(format!(
                "[get_candlesticks: no data returned for {ticker} from Tushare. \
                 Verify the ts_code format.]"
            ));
        }

        let idx = |name: &str| fields.iter().position(|f| f == name);
        let i_date = idx("trade_date");
        let i_open = idx("open");
        let i_high = idx("high");
        let i_low = idx("low");
        let i_close = idx("close");
        let i_vol = idx("vol");
        let i_pct = idx("pct_chg");

        let cell = |row: &Vec<serde_json::Value>, i: Option<usize>| -> String {
            i.and_then(|j| row.get(j))
                .map(|v| match v {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };

        let mut rows: Vec<[String; 7]> = items
            .iter()
            .map(|row| {
                let raw_date = cell(row, i_date);
                let date = tushare_date_fmt(&raw_date);
                let open = cell(row, i_open);
                let high = cell(row, i_high);
                let low = cell(row, i_low);
                let close = cell(row, i_close);
                let pct = cell(row, i_pct);
                let vol = cell(row, i_vol)
                    .parse::<f64>()
                    .map(fmt_vol_cn)
                    .unwrap_or_default();
                [date, open, high, low, close, pct, vol]
            })
            .collect();

        rows.sort_by(|a, b| a[0].cmp(&b[0]));

        let adj_label = if adjusted { "前复权" } else { "不复权" };
        let mut out = format!(
            "## {ticker} · {} · {} ({limit})\n\n",
            interval.label_cn(),
            adj_label
        );
        out.push_str("| 日期       | 开盘    | 最高    | 最低    | 收盘    | 涨跌幅 | 成交量   |\n");
        out.push_str("|-----------|---------|---------|---------|---------|--------|----------|\n");
        for r in &rows {
            out.push_str(&format!(
                "| {:<10} | {:>7} | {:>7} | {:>7} | {:>7} | {:>6} | {:>8} |\n",
                r[0], r[1], r[2], r[3], r[4], r[5], r[6]
            ));
        }
        out.push_str("\n_来源: Tushare Pro (pro_bar)_\n");
        Ok(out)
    }

    async fn fetch_yahoo(
        &self,
        ticker: &str,
        interval: Interval,
        limit: usize,
        adjusted: bool,
        cancel: CancellationToken,
    ) -> Result<String> {
        let range = if limit <= 60 {
            "3mo"
        } else if limit <= 130 {
            "6mo"
        } else {
            "1y"
        };

        let url = format!(
            "{YAHOO_BASE}/{ticker}?interval={}&range={range}",
            interval.yahoo_interval()
        );

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before yahoo request"),
            r = self.http.get(&url).send() => r?,
        };
        if !resp.status().is_success() {
            bail!("Yahoo Finance returned HTTP {}", resp.status().as_u16());
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during yahoo parse"),
            r = resp.json() => r?,
        };

        let result = body
            .pointer("/chart/result/0")
            .ok_or_else(|| anyhow!("Yahoo Finance: unexpected response shape"))?;

        let symbol = result
            .pointer("/meta/symbol")
            .and_then(|v| v.as_str())
            .unwrap_or(ticker);

        let timestamps = result
            .pointer("/timestamp")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("Yahoo Finance: missing timestamps"))?;

        let quote = result
            .pointer("/indicators/quote/0")
            .ok_or_else(|| anyhow!("Yahoo Finance: missing quote data"))?;

        let opens = array_f64(quote, "open");
        let highs = array_f64(quote, "high");
        let lows = array_f64(quote, "low");
        let closes_raw = array_f64(quote, "close");
        let volumes = array_f64(quote, "volume");

        let adj_closes = if adjusted {
            result
                .pointer("/indicators/adjclose/0/adjclose")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().map(|v| v.as_f64()).collect::<Vec<_>>())
        } else {
            None
        };

        let closes: Vec<Option<f64>> = if let Some(adj) = adj_closes {
            adj
        } else {
            closes_raw
        };

        struct Row {
            date: String,
            open: f64,
            high: f64,
            low: f64,
            close: f64,
            volume: f64,
        }

        let mut rows: Vec<Row> = timestamps
            .iter()
            .enumerate()
            .filter_map(|(i, ts)| {
                let ts = ts.as_i64()?;
                let open = opens.get(i)?.as_ref()?;
                let high = highs.get(i)?.as_ref()?;
                let low = lows.get(i)?.as_ref()?;
                let close = closes.get(i)?.as_ref()?;
                let volume = volumes.get(i).and_then(|v| v.as_ref()).copied().unwrap_or(0.0);
                let date = DateTime::from_timestamp(ts, 0)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string();
                Some(Row {
                    date,
                    open: *open,
                    high: *high,
                    low: *low,
                    close: *close,
                    volume,
                })
            })
            .collect();

        rows.sort_by(|a, b| a.date.cmp(&b.date));
        if rows.len() > limit {
            rows.drain(..rows.len() - limit);
        }

        let adj_label = if adjusted { "Adj" } else { "Raw" };
        let mut out = format!(
            "## {symbol} · {} · {adj_label} ({limit})\n\n",
            interval.label()
        );
        out.push_str("| Date       | Open    | High    | Low     | Close   | Volume   |\n");
        out.push_str("|-----------|---------|---------|---------|---------|----------|\n");
        for r in &rows {
            out.push_str(&format!(
                "| {:<10} | {:>7.2} | {:>7.2} | {:>7.2} | {:>7.2} | {:>8} |\n",
                r.date,
                r.open,
                r.high,
                r.low,
                r.close,
                fmt_vol(r.volume)
            ));
        }
        out.push_str("\n_Source: Yahoo Finance_\n");
        Ok(out)
    }

    async fn fetch_binance(
        &self,
        ticker: &str,
        interval: Interval,
        limit: usize,
        cancel: CancellationToken,
    ) -> Result<String> {
        let symbol = ticker.to_uppercase();
        let url = format!(
            "{BINANCE_BASE}?symbol={symbol}&interval={}&limit={limit}",
            interval.binance_interval()
        );

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before binance request"),
            r = self.http.get(&url).send() => r?,
        };
        if !resp.status().is_success() {
            bail!("Binance returned HTTP {}", resp.status().as_u16());
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during binance parse"),
            r = resp.json() => r?,
        };

        let klines = body
            .as_array()
            .ok_or_else(|| anyhow!("Binance: unexpected response shape"))?;

        if klines.is_empty() {
            return Ok(format!(
                "[get_candlesticks: no data returned for {symbol} from Binance. \
                 Verify the symbol format (e.g. BTCUSDT).]"
            ));
        }

        struct Row {
            date: String,
            open: f64,
            high: f64,
            low: f64,
            close: f64,
            volume: f64,
        }

        let parse_str_f64 = |v: &serde_json::Value| -> Option<f64> {
            v.as_str()?.parse().ok()
        };

        let mut rows: Vec<Row> = klines
            .iter()
            .filter_map(|k| {
                let arr = k.as_array()?;
                let ts_ms = arr.first()?.as_i64()?;
                let open = parse_str_f64(arr.get(1)?)?;
                let high = parse_str_f64(arr.get(2)?)?;
                let low = parse_str_f64(arr.get(3)?)?;
                let close = parse_str_f64(arr.get(4)?)?;
                let volume = parse_str_f64(arr.get(5)?).unwrap_or(0.0);
                let date = DateTime::from_timestamp(ts_ms / 1000, 0)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string();
                Some(Row { date, open, high, low, close, volume })
            })
            .collect();

        rows.sort_by(|a, b| a.date.cmp(&b.date));

        let base = symbol.trim_end_matches("USDT").trim_end_matches("BTC");
        let mut out = format!(
            "## {base}/USDT · {} · Binance ({limit})\n\n",
            interval.label()
        );
        out.push_str("| Date       | Open        | High        | Low         | Close       | Volume       |\n");
        out.push_str("|-----------|-------------|-------------|-------------|-------------|-------------|\n");
        for r in &rows {
            out.push_str(&format!(
                "| {:<10} | {:>11} | {:>11} | {:>11} | {:>11} | {:>12} |\n",
                r.date,
                fmt_crypto_price(r.open),
                fmt_crypto_price(r.high),
                fmt_crypto_price(r.low),
                fmt_crypto_price(r.close),
                fmt_vol(r.volume)
            ));
        }
        out.push_str("\n_Source: Binance_\n");
        Ok(out)
    }
}

fn array_f64(v: &serde_json::Value, key: &str) -> Vec<Option<f64>> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().map(|x| x.as_f64()).collect())
        .unwrap_or_default()
}

fn tushare_date_fmt(s: &str) -> String {
    if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..])
    } else {
        s.to_string()
    }
}

fn fmt_vol(v: f64) -> String {
    if v >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{:.0}", v)
    }
}

fn fmt_vol_cn(v: f64) -> String {
    if v >= 1e9 {
        format!("{:.1}B手", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.1}M手", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1}k手", v / 1e3)
    } else {
        format!("{:.0}手", v)
    }
}

fn fmt_crypto_price(p: f64) -> String {
    if p < 1.0 {
        format!("{:.6}", p)
    } else {
        format!("{:.2}", p)
    }
}
