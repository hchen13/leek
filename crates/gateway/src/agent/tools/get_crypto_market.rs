use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "get_crypto_market";
const REQUEST_TIMEOUT_SECS: u64 = 20;

pub struct GetCryptoMarketTool {
    http: Client,
}

impl GetCryptoMarketTool {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent("leek-research/1.0")
            .build()?;
        Ok(Self { http })
    }

    async fn fetch_global(&self, vs_currency: &str, cancel: &CancellationToken) -> Result<String> {
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get("https://api.coingecko.com/api/v3/global").send() => r?,
        };
        if !resp.status().is_success() {
            bail!("CoinGecko /global returned HTTP {}", resp.status().as_u16());
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = resp.json() => r?,
        };
        let data = body
            .get("data")
            .ok_or_else(|| anyhow!("missing 'data' in CoinGecko global response"))?;

        let total_market_cap = data
            .get("total_market_cap")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let total_volume = data
            .get("total_volume")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let btc_dominance = data
            .get("market_cap_percentage")
            .and_then(|v| v.get("btc"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let eth_dominance = data
            .get("market_cap_percentage")
            .and_then(|v| v.get("eth"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let change_24h = data
            .get("market_cap_change_percentage_24h_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let mut out = String::from("## Crypto Market Overview\n\n");
        out.push_str(&format!(
            "- Total Market Cap: ${:.2}T\n",
            total_market_cap / 1_000_000_000_000.0
        ));
        out.push_str(&format!("- 24h Change: {:+.2}%\n", change_24h));
        out.push_str(&format!(
            "- 24h Volume: ${:.2}B\n",
            total_volume / 1_000_000_000.0
        ));
        out.push_str(&format!("- BTC Dominance: {:.1}%\n", btc_dominance));
        out.push_str(&format!("- ETH Dominance: {:.1}%\n", eth_dominance));
        out.push_str("\n_Source: CoinGecko_\n");
        Ok(out)
    }

    async fn fetch_top_coins(
        &self,
        vs_currency: &str,
        limit: u64,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let per_page = limit.clamp(1, 50);
        let url = format!(
            "https://api.coingecko.com/api/v3/coins/markets?vs_currency={vs_currency}&order=market_cap_desc&per_page={per_page}&page=1&sparkline=false"
        );
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&url).send() => r?,
        };
        if !resp.status().is_success() {
            bail!(
                "CoinGecko /coins/markets returned HTTP {}",
                resp.status().as_u16()
            );
        }
        let coins: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = resp.json() => r?,
        };

        let arr = coins
            .as_array()
            .ok_or_else(|| anyhow!("unexpected CoinGecko response shape"))?;

        let mut out = format!(
            "## Top {} Coins by Market Cap ({vs_currency})\n\n",
            arr.len()
        );
        out.push_str("| # | Name | Symbol | Price | 24h% | Market Cap | Volume |\n");
        out.push_str("|---|------|--------|-------|------|------------|--------|\n");

        for (i, coin) in arr.iter().enumerate() {
            let name = coin.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let symbol = coin
                .get("symbol")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_uppercase();
            let price = coin
                .get("current_price")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let change = coin
                .get("price_change_percentage_24h")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let mcap = coin
                .get("market_cap")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let vol = coin
                .get("total_volume")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            out.push_str(&format!(
                "| {} | {} | {} | {} | {:+.2}% | {} | {} |\n",
                i + 1,
                name,
                symbol,
                fmt_price(price),
                change,
                fmt_large(mcap),
                fmt_large(vol),
            ));
        }
        out.push_str("\n_Source: CoinGecko_\n");
        Ok(out)
    }

    async fn fetch_coin_detail(
        &self,
        coin_id: &str,
        vs_currency: &str,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let url = format!(
            "https://api.coingecko.com/api/v3/coins/{coin_id}?localization=false&tickers=false&market_data=true&community_data=false&developer_data=false"
        );
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = self.http.get(&url).send() => r?,
        };
        if !resp.status().is_success() {
            bail!(
                "CoinGecko /coins/{coin_id} returned HTTP {}",
                resp.status().as_u16()
            );
        }
        let body: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted"),
            r = resp.json() => r?,
        };

        let name = body.get("name").and_then(|v| v.as_str()).unwrap_or(coin_id);
        let symbol = body
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_uppercase();
        let md = body
            .get("market_data")
            .ok_or_else(|| anyhow!("missing market_data"))?;

        let price = md
            .get("current_price")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let mcap = md
            .get("market_cap")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let vol = md
            .get("total_volume")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let chg_24h = md
            .get("price_change_percentage_24h")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let chg_7d = md
            .get("price_change_percentage_7d")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let ath = md
            .get("ath")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let ath_chg = md
            .get("ath_change_percentage")
            .and_then(|v| v.get(vs_currency))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let supply = md
            .get("circulating_supply")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let mut out = format!("## {name} ({symbol})\n\n");
        out.push_str(&format!(
            "- Price: {} {}\n",
            fmt_price(price),
            vs_currency.to_uppercase()
        ));
        out.push_str(&format!("- 24h Change: {:+.2}%\n", chg_24h));
        out.push_str(&format!("- 7d Change: {:+.2}%\n", chg_7d));
        out.push_str(&format!("- Market Cap: {}\n", fmt_large(mcap)));
        out.push_str(&format!("- 24h Volume: {}\n", fmt_large(vol)));
        out.push_str(&format!(
            "- ATH: {} ({:+.1}% from ATH)\n",
            fmt_price(ath),
            ath_chg
        ));
        out.push_str(&format!("- Circulating Supply: {:.0} {symbol}\n", supply));
        out.push_str("\n_Source: CoinGecko_\n");
        Ok(out)
    }
}

fn fmt_price(p: f64) -> String {
    if p >= 1000.0 {
        format!("${:.2}", p)
    } else if p >= 1.0 {
        format!("${:.4}", p)
    } else {
        format!("${:.8}", p)
    }
}

fn fmt_large(v: f64) -> String {
    if v >= 1_000_000_000.0 {
        format!("${:.2}B", v / 1_000_000_000.0)
    } else if v >= 1_000_000.0 {
        format!("${:.2}M", v / 1_000_000.0)
    } else {
        format!("${:.0}", v)
    }
}

#[async_trait]
impl ToolHandler for GetCryptoMarketTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch cryptocurrency market context from configured public market-data sources. \
                Use when the task needs market regime, dominance, liquidity, or coin-level spot context before reasoning about crypto assets. \
                This is market evidence, not token valuation or an investment conclusion. \
                Supports total market overview, top coins by market cap, and individual coin stats such as price, volume, 24h change, and ATH."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["global", "top_coins", "coin_detail"],
                        "description": "global=market overview; top_coins=top N by market cap; coin_detail=specific coin stats"
                    },
                    "coin_id": {
                        "type": "string",
                        "description": "CoinGecko coin ID for coin_detail mode, e.g. 'bitcoin', 'ethereum', 'solana'"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "For top_coins: number of coins (default 20, max 50)"
                    },
                    "vs_currency": {
                        "type": "string",
                        "description": "Quote currency (default 'usd')"
                    }
                },
                "required": ["mode"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let mode = args
            .get("mode")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'mode' argument"))?;
        let vs_currency = args
            .get("vs_currency")
            .and_then(|v| v.as_str())
            .unwrap_or("usd");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);

        match mode {
            "global" => self.fetch_global(vs_currency, &cancel).await,
            "top_coins" => self.fetch_top_coins(vs_currency, limit, &cancel).await,
            "coin_detail" => {
                let coin_id = args
                    .get("coin_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("'coin_id' is required for coin_detail mode"))?;
                self.fetch_coin_detail(coin_id, vs_currency, &cancel).await
            }
            other => bail!("unknown mode '{other}' — use 'global', 'top_coins', or 'coin_detail'"),
        }
    }
}
