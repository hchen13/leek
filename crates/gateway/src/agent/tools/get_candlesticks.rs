use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::DateTime;
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::{data_provider_tokens, ToolContext, ToolHandler};

const TOOL_NAME: &str = "get_candlesticks";
const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";
const TUSHARE_FIELDS: &str = "ts_code,trade_date,open,high,low,close,vol,amount,pct_chg";
const TUSHARE_ADJ_FIELDS: &str = "ts_code,trade_date,adj_factor";
const YAHOO_BASE: &str = "https://query1.finance.yahoo.com/v8/finance/chart";
const BINANCE_BASE: &str = "https://api.binance.com/api/v3/klines";
const COINGECKO_SEARCH: &str = "https://api.coingecko.com/api/v3/search";
const COINGECKO_OHLC: &str = "https://api.coingecko.com/api/v3/coins/{id}/ohlc";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 200;
const MAX_MODEL_TABLE_ROWS: usize = 40;

pub struct GetCandlesticksTool {
    http: Client,
}

impl GetCandlesticksTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
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
    #[serde(rename = "15m")]
    Minute15,
    #[serde(rename = "1d")]
    #[default]
    Day,
    #[serde(rename = "1w")]
    Week,
    #[serde(rename = "1mo")]
    Month,
}

impl Interval {
    fn tushare_api(&self) -> &str {
        match self {
            Interval::Minute15 => "stk_mins",
            Interval::Day => "daily",
            Interval::Week => "weekly",
            Interval::Month => "monthly",
        }
    }

    fn yahoo_interval(&self) -> &str {
        match self {
            Interval::Minute15 => "15m",
            Interval::Day => "1d",
            Interval::Week => "1wk",
            Interval::Month => "1mo",
        }
    }

    fn binance_interval(&self) -> &str {
        match self {
            Interval::Minute15 => "15m",
            Interval::Day => "1d",
            Interval::Week => "1w",
            Interval::Month => "1M",
        }
    }

    fn label(&self) -> &str {
        match self {
            Interval::Minute15 => "15m",
            Interval::Day => "Daily",
            Interval::Week => "Weekly",
            Interval::Month => "Monthly",
        }
    }

    fn label_cn(&self) -> &str {
        match self {
            Interval::Minute15 => "15分钟",
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
            description: "Fetch historical OHLCV (K-line) data for supported asset classes.\n\
                Market is controlled by the `market` parameter — this single tool covers A-share, US/HK stocks, and crypto.\n\
                - A-share (CN equities): ts_code like \"600519.SH\", \"000001.SZ\", \"300750.SZ\"\n\
                - US/HK stocks: ticker like \"AAPL\", \"TSLA\", \"0700.HK\", \"BABA\"\n\
                - Crypto: Binance symbol like \"BTCUSDT\", \"ETHUSDT\", \"SOLUSDT\"\n\
                Interval: 1d/1w/1mo for all markets. A-share daily/weekly/monthly uses the configured official historical K-line source; same-day daily candles may lag after market close. 15m is only for US/HK/crypto until an A-share intraday source is wired.\n\
                Returns a compact summary plus a recent markdown OHLCV table sorted oldest→newest; \
                front-end cards can fetch a larger interactive series separately.\n\
                Prefer this whenever price trend or mean-reversion context matters; use get_a_share_market_snapshot with freshness=realtime or both for current-session A-share quotes."
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
                        "enum": ["15m", "1d", "1w", "1mo"],
                        "description": "Candle interval, default 1d"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of candles (default 30, max 200)"
                    },
                    "adjusted": {
                        "type": "boolean",
                        "description": "Adjusted prices where supported. A-share applies front-adjustment to OHLC when all returned rows have factors; US/HK uses Yahoo adjclose for close only; unsupported markets return raw prices."
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
        ctx: &ToolContext,
    ) -> Result<String> {
        let args: Args =
            serde_json::from_value(args).map_err(|e| anyhow!("invalid arguments: {e}"))?;
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let adjusted = args.adjusted.unwrap_or(true);

        match args.market {
            Market::AShare => {
                self.fetch_a_share(ctx, &args.ticker, args.interval, limit, adjusted, cancel)
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
        ctx: &ToolContext,
        ticker: &str,
        interval: Interval,
        limit: usize,
        adjusted: bool,
        cancel: CancellationToken,
    ) -> Result<String> {
        if matches!(interval, Interval::Minute15) {
            return Ok("[get_candlesticks: A-share 15m candles are not available through the current primary K-line provider. Use Tushare daily/weekly/monthly for historical K-lines, or get_a_share_market_snapshot freshness=realtime/both for the current-session quote snapshot.]".to_string());
        }

        let token = data_provider_tokens::tushare_token(ctx).await?;
        let name = super::ashare_security_name(&self.http, &token, ticker, &cancel).await;
        let label = name
            .map(|n| format!("{n}（{ticker}）"))
            .unwrap_or_else(|| ticker.to_string());

        let payload = serde_json::json!({
            "api_name": interval.tushare_api(),
            "token": token,
            "params": {
                "ts_code": ticker,
                "limit": limit,
            },
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
                "[get_candlesticks: no data returned for {ticker} from Tushare. Treat this as a valid empty/source-coverage gap for the selected market and interval, not proof of no trading; verify the symbol only if other evidence suggests a code mismatch.]"
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

        let mut rows: Vec<CandleRow> = items
            .iter()
            .filter_map(|row| {
                let raw_date = cell(row, i_date);
                let date = tushare_date_fmt(&raw_date);
                Some(CandleRow {
                    date,
                    open: cell(row, i_open).parse().ok()?,
                    high: cell(row, i_high).parse().ok()?,
                    low: cell(row, i_low).parse().ok()?,
                    close: cell(row, i_close).parse().ok()?,
                    pct_chg: cell(row, i_pct).parse().unwrap_or(0.0),
                    volume: cell(row, i_vol).parse().unwrap_or(0.0),
                })
            })
            .collect();

        rows.sort_by(|a, b| a.date.cmp(&b.date));

        let mut price_basis = "不复权";
        let mut basis_note = String::new();
        if adjusted {
            match self
                .fetch_a_share_adj_factors(&token, ticker, limit, &cancel)
                .await
            {
                Ok(factors) if apply_front_adjustment(&mut rows, &factors) => {
                    price_basis = "前复权";
                }
                Ok(_) => {
                    price_basis = "不复权（前复权不可用）";
                    basis_note =
                        "\n_复权说明: 已请求前复权，但未匹配到足够 adj_factor；本次返回不复权价格。_"
                            .to_string();
                }
                Err(err) => {
                    price_basis = "不复权（前复权失败）";
                    basis_note = format!(
                        "\n_复权说明: 已请求前复权，但 adj_factor 获取失败；本次返回不复权价格。错误: {}_",
                        preview_err(&err.to_string())
                    );
                }
            }
        }

        let mut out = format!(
            "## {label} · {} · {price_basis} ({limit})\n\n",
            interval.label_cn()
        );
        out.push_str(&render_candle_summary(&rows, interval.label_cn()));
        out.push('\n');
        out.push_str(
            "| 日期       | 开盘    | 最高    | 最低    | 收盘    | 涨跌幅 | 成交量   |\n",
        );
        out.push_str("|-----------|---------|---------|---------|---------|--------|----------|\n");
        let display_rows = recent_rows(&rows, MAX_MODEL_TABLE_ROWS);
        for r in display_rows {
            out.push_str(&format!(
                "| {:<10} | {:>7} | {:>7} | {:>7} | {:>7} | {:>6} | {:>8} |\n",
                r.date,
                fmt_price(r.open),
                fmt_price(r.high),
                fmt_price(r.low),
                fmt_price(r.close),
                fmt_pct(r.pct_chg),
                fmt_vol_cn(r.volume)
            ));
        }
        if rows.len() > display_rows.len() {
            out.push_str(&format!(
                "\n_表格仅保留最近 {} 根，完整 K 线由前端卡片按需加载。_",
                display_rows.len()
            ));
        }
        out.push_str(&basis_note);
        out.push_str("\n_来源: 配置的 A 股日/周/月 K 线来源。同日收盘 K 线可能在收盘后一段时间才更新；当前盘中价请用 A 股行情快照工具的 realtime/both。_\n");
        Ok(out)
    }

    async fn fetch_a_share_adj_factors(
        &self,
        token: &str,
        ticker: &str,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Vec<(String, f64)>> {
        let payload = serde_json::json!({
            "api_name": "adj_factor",
            "token": token,
            "params": {
                "ts_code": ticker,
                "limit": limit,
            },
            "fields": TUSHARE_ADJ_FIELDS,
        });
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before tushare adj_factor request"),
            r = self.http.post(TUSHARE_ENDPOINT).json(&payload).send() => r?,
        };
        if !resp.status().is_success() {
            bail!(
                "tushare adj_factor returned HTTP {}",
                resp.status().as_u16()
            );
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during tushare adj_factor parse"),
            r = resp.json() => r?,
        };
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            bail!("tushare adj_factor error (code={code}): {msg}");
        }
        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("tushare adj_factor response missing 'data'"))?;
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
        let idx = |name: &str| fields.iter().position(|f| f == name);
        let i_date = idx("trade_date");
        let i_factor = idx("adj_factor");
        let cell = |row: &Vec<serde_json::Value>, i: Option<usize>| -> String {
            i.and_then(|j| row.get(j))
                .map(|v| match v {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };
        let mut factors: Vec<(String, f64)> = items
            .iter()
            .filter_map(|row| {
                let date = tushare_date_fmt(&cell(row, i_date));
                let factor = cell(row, i_factor).parse().ok()?;
                Some((date, factor))
            })
            .collect();
        factors.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(factors)
    }

    async fn fetch_yahoo(
        &self,
        ticker: &str,
        interval: Interval,
        limit: usize,
        adjusted: bool,
        cancel: CancellationToken,
    ) -> Result<String> {
        let range = if matches!(interval, Interval::Minute15) {
            "60d"
        } else if limit <= 60 {
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
        let yahoo_name = result
            .pointer("/meta/shortName")
            .or_else(|| result.pointer("/meta/longName"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let label = yahoo_name
            .map(|n| format!("{n}（{symbol}）"))
            .unwrap_or_else(|| symbol.to_string());

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
                let volume = volumes
                    .get(i)
                    .and_then(|v| v.as_ref())
                    .copied()
                    .unwrap_or(0.0);
                let fmt = if matches!(interval, Interval::Minute15) {
                    "%Y-%m-%d %H:%M"
                } else {
                    "%Y-%m-%d"
                };
                let date = DateTime::from_timestamp(ts, 0)
                    .unwrap()
                    .format(fmt)
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
            "## {label} · {} · {adj_label} ({limit})\n\n",
            interval.label()
        );
        let date_head = if matches!(interval, Interval::Minute15) {
            "Date             "
        } else {
            "Date       "
        };
        let sep = if matches!(interval, Interval::Minute15) {
            "------------------"
        } else {
            "-----------"
        };
        out.push_str(&format!(
            "| {date_head} | Open    | High    | Low     | Close   | Volume   |\n"
        ));
        out.push_str(&format!(
            "|{sep}|---------|---------|---------|---------|----------|\n"
        ));
        for r in &rows {
            out.push_str(&format!(
                "| {:<16} | {:>7.2} | {:>7.2} | {:>7.2} | {:>7.2} | {:>8} |\n",
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
        // Normalise: uppercase, append USDT if no quote currency present.
        let raw = ticker.to_uppercase();
        let symbol = if raw.ends_with("USDT")
            || raw.ends_with("BTC")
            || raw.ends_with("ETH")
            || raw.ends_with("BNB")
        {
            raw.clone()
        } else {
            format!("{raw}USDT")
        };

        let url = format!(
            "{BINANCE_BASE}?symbol={symbol}&interval={}&limit={limit}",
            interval.binance_interval()
        );

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before binance request"),
            r = self.http.get(&url).send() => r?,
        };
        let status = resp.status();
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during binance parse"),
            r = resp.json() => r?,
        };

        // 400 typically means the symbol doesn't exist on Binance.
        // Fall back to CoinGecko for less-liquid tokens.
        if !status.is_success() {
            let binance_msg = body
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            tracing::info!(symbol, %binance_msg, "Binance klines failed, trying CoinGecko");
            return self
                .fetch_coingecko(&raw, interval, limit, cancel)
                .await
                .map_err(|cg_err| {
                    anyhow!(
                        "Binance: {} (symbol={symbol}); CoinGecko fallback also failed: {cg_err}",
                        binance_msg
                    )
                });
        }

        let klines = body
            .as_array()
            .ok_or_else(|| anyhow!("Binance: unexpected response shape"))?;

        if klines.is_empty() {
            return self.fetch_coingecko(&raw, interval, limit, cancel).await;
        }

        struct Row {
            date: String,
            open: f64,
            high: f64,
            low: f64,
            close: f64,
            volume: f64,
        }

        let parse_str_f64 = |v: &serde_json::Value| -> Option<f64> { v.as_str()?.parse().ok() };

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
                let fmt = if matches!(interval, Interval::Minute15) {
                    "%Y-%m-%d %H:%M"
                } else {
                    "%Y-%m-%d"
                };
                let date = DateTime::from_timestamp(ts_ms / 1000, 0)
                    .unwrap()
                    .format(fmt)
                    .to_string();
                Some(Row {
                    date,
                    open,
                    high,
                    low,
                    close,
                    volume,
                })
            })
            .collect();

        rows.sort_by(|a, b| a.date.cmp(&b.date));

        let base = symbol
            .trim_end_matches("USDT")
            .trim_end_matches("BTC")
            .trim_end_matches("ETH");
        let mut out = format!(
            "## {base}/USDT · {} · Binance ({limit})\n\n",
            interval.label()
        );
        let date_head = if matches!(interval, Interval::Minute15) {
            "Date             "
        } else {
            "Date       "
        };
        let sep = if matches!(interval, Interval::Minute15) {
            "------------------"
        } else {
            "-----------"
        };
        out.push_str(&format!(
            "| {date_head} | Open        | High        | Low         | Close       | Volume       |\n"
        ));
        out.push_str(&format!(
            "|{sep}|-------------|-------------|-------------|-------------|-------------|\n",
        ));
        for r in &rows {
            out.push_str(&format!(
                "| {:<16} | {:>11} | {:>11} | {:>11} | {:>11} | {:>12} |\n",
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

    /// CoinGecko fallback for tokens not listed on Binance.
    /// Uses /search to resolve symbol → coin_id, then /coins/{id}/ohlc.
    async fn fetch_coingecko(
        &self,
        symbol: &str,
        interval: Interval,
        limit: usize,
        cancel: CancellationToken,
    ) -> Result<String> {
        let base = symbol
            .to_uppercase()
            .trim_end_matches("USDT")
            .trim_end_matches("BTC")
            .trim_end_matches("ETH")
            .to_string();

        // Step 1: resolve symbol → CoinGecko coin_id.
        let search_url = format!("{COINGECKO_SEARCH}?query={base}");
        let search_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before coingecko search"),
            r = self.http.get(&search_url).send() => r?,
        };
        let search_body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during coingecko search parse"),
            r = search_resp.json() => r?,
        };

        let coin_id = search_body
            .pointer("/coins/0/id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("CoinGecko: no coin found for symbol '{base}'"))?
            .to_string();

        // Step 2: OHLC. CoinGecko granularity: ≤2d=30min, ≤30d=4h, else=daily.
        // To get `limit` daily candles we request enough days.
        let days = match interval {
            Interval::Minute15 => "2".to_string(),
            Interval::Day => limit.max(90).to_string(),
            Interval::Week => (limit * 7).max(90).to_string(),
            Interval::Month => "max".to_string(),
        };
        let ohlc_url =
            COINGECKO_OHLC.replace("{id}", &coin_id) + &format!("?vs_currency=usd&days={days}");

        let ohlc_resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted before coingecko ohlc"),
            r = self.http.get(&ohlc_url).send() => r?,
        };
        if !ohlc_resp.status().is_success() {
            bail!(
                "CoinGecko OHLC returned HTTP {}",
                ohlc_resp.status().as_u16()
            );
        }
        let ohlc_body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during coingecko ohlc parse"),
            r = ohlc_resp.json() => r?,
        };

        let candles = ohlc_body
            .as_array()
            .ok_or_else(|| anyhow!("CoinGecko: unexpected OHLC shape"))?;

        if candles.is_empty() {
            bail!("CoinGecko: no OHLC data for '{coin_id}'");
        }

        struct Row {
            date: String,
            open: f64,
            high: f64,
            low: f64,
            close: f64,
        }

        let mut rows: Vec<Row> = candles
            .iter()
            .filter_map(|c| {
                let arr = c.as_array()?;
                let ts_ms = arr.first()?.as_i64()?;
                let open = arr.get(1)?.as_f64()?;
                let high = arr.get(2)?.as_f64()?;
                let low = arr.get(3)?.as_f64()?;
                let close = arr.get(4)?.as_f64()?;
                let date = DateTime::from_timestamp(ts_ms / 1000, 0)
                    .unwrap()
                    .format("%Y-%m-%d")
                    .to_string();
                Some(Row {
                    date,
                    open,
                    high,
                    low,
                    close,
                })
            })
            .collect();

        rows.sort_by(|a, b| a.date.cmp(&b.date));
        // Deduplicate by date (CoinGecko 4h candles → multiple per day).
        rows.dedup_by(|a, b| {
            if a.date == b.date {
                b.close = a.close;
                true
            } else {
                false
            }
        });
        if rows.len() > limit {
            rows.drain(..rows.len() - limit);
        }

        let mut out = format!(
            "## {base}/USD · {} · CoinGecko ({limit})\n\n",
            interval.label()
        );
        out.push_str("| Date       | Open        | High        | Low         | Close       |\n");
        out.push_str("|-----------|-------------|-------------|-------------|-------------|\n");
        for r in &rows {
            out.push_str(&format!(
                "| {:<10} | {:>11} | {:>11} | {:>11} | {:>11} |\n",
                r.date,
                fmt_crypto_price(r.open),
                fmt_crypto_price(r.high),
                fmt_crypto_price(r.low),
                fmt_crypto_price(r.close),
            ));
        }
        out.push_str("\n_Source: CoinGecko_\n");
        Ok(out)
    }
}

fn array_f64(v: &serde_json::Value, key: &str) -> Vec<Option<f64>> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().map(|x| x.as_f64()).collect())
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
struct CandleRow {
    date: String,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    pct_chg: f64,
    volume: f64,
}

fn apply_front_adjustment(rows: &mut [CandleRow], factors: &[(String, f64)]) -> bool {
    if rows.is_empty() || factors.is_empty() {
        return false;
    }
    let factor_by_date: std::collections::HashMap<&str, f64> = factors
        .iter()
        .map(|(date, factor)| (date.as_str(), *factor))
        .collect();
    let Some(latest_factor) = rows
        .iter()
        .rev()
        .find_map(|row| factor_by_date.get(row.date.as_str()).copied())
    else {
        return false;
    };
    if latest_factor <= 0.0 {
        return false;
    }
    let Some(ratios) = rows
        .iter()
        .map(|row| {
            let factor = factor_by_date.get(row.date.as_str()).copied()?;
            if factor <= 0.0 {
                return None;
            }
            Some(factor / latest_factor)
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    for (row, ratio) in rows.iter_mut().zip(ratios) {
        row.open *= ratio;
        row.high *= ratio;
        row.low *= ratio;
        row.close *= ratio;
    }
    true
}

fn render_candle_summary(rows: &[CandleRow], interval_label: &str) -> String {
    let Some(first) = rows.first() else {
        return "_摘要: 暂无可计算 K 线。_\n".to_string();
    };
    let Some(last) = rows.last() else {
        return "_摘要: 暂无可计算 K 线。_\n".to_string();
    };
    let high = rows
        .iter()
        .map(|r| r.high)
        .fold(f64::NEG_INFINITY, f64::max);
    let low = rows.iter().map(|r| r.low).fold(f64::INFINITY, f64::min);
    let change = if first.close.abs() > f64::EPSILON {
        (last.close / first.close - 1.0) * 100.0
    } else {
        0.0
    };
    let percentile = if high > low {
        ((last.close - low) / (high - low) * 100.0).clamp(0.0, 100.0)
    } else {
        50.0
    };
    let drawdown = max_drawdown_pct(rows);
    format!(
        "_摘要: {interval_label}共{}根，区间{}→{}，最新收盘{}，区间涨跌{}，区间高低{} / {}，当前位置约为区间{}分位，最大回撤{}。_\n",
        rows.len(),
        first.date,
        last.date,
        fmt_price(last.close),
        fmt_pct(change),
        fmt_price(high),
        fmt_price(low),
        fmt_pct(percentile),
        fmt_pct(drawdown)
    )
}

fn max_drawdown_pct(rows: &[CandleRow]) -> f64 {
    let mut peak = f64::NEG_INFINITY;
    let mut max_dd = 0.0;
    for row in rows {
        peak = peak.max(row.high);
        if peak > 0.0 {
            let dd = (row.close / peak - 1.0) * 100.0;
            if dd < max_dd {
                max_dd = dd;
            }
        }
    }
    max_dd
}

fn recent_rows(rows: &[CandleRow], max_rows: usize) -> &[CandleRow] {
    if rows.len() <= max_rows {
        rows
    } else {
        &rows[rows.len() - max_rows..]
    }
}

fn fmt_price(v: f64) -> String {
    format!("{v:.2}")
}

fn fmt_pct(v: f64) -> String {
    format!("{v:+.2}%")
}

fn preview_err(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= 120 {
        trimmed.to_string()
    } else {
        let mut end = 120;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &trimmed[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(date: &str, close: f64) -> CandleRow {
        CandleRow {
            date: date.to_string(),
            open: close,
            high: close * 1.02,
            low: close * 0.98,
            close,
            pct_chg: 0.0,
            volume: 1000.0,
        }
    }

    #[test]
    fn front_adjustment_keeps_latest_price_anchored() {
        let mut rows = vec![row("2026-01-01", 10.0), row("2026-01-02", 20.0)];
        let factors = vec![
            ("2026-01-01".to_string(), 2.0),
            ("2026-01-02".to_string(), 4.0),
        ];

        assert!(apply_front_adjustment(&mut rows, &factors));
        assert_eq!(fmt_price(rows[0].close), "5.00");
        assert_eq!(fmt_price(rows[1].close), "20.00");
    }

    #[test]
    fn candle_summary_contains_high_signal_metrics() {
        let rows = vec![row("2026-01-01", 10.0), row("2026-01-02", 12.0)];
        let summary = render_candle_summary(&rows, "日线");

        assert!(summary.contains("日线共2根"));
        assert!(summary.contains("区间涨跌"));
        assert!(summary.contains("最大回撤"));
    }

    #[test]
    fn recent_rows_limits_model_table() {
        let rows: Vec<CandleRow> = (0..45)
            .map(|i| row(&format!("2026-01-{i:02}"), 10.0 + i as f64))
            .collect();
        let recent = recent_rows(&rows, MAX_MODEL_TABLE_ROWS);

        assert_eq!(recent.len(), MAX_MODEL_TABLE_ROWS);
        assert_eq!(recent[0].date, "2026-01-05");
    }
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
