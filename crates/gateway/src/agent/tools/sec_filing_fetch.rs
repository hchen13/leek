//! `sec_filing_fetch` — list recent SEC EDGAR filings for a US ticker.
//!
//! Free + public API. Two-step flow:
//!   1. ticker → CIK via https://www.sec.gov/files/company_tickers.json
//!   2. filings list via https://data.sec.gov/submissions/CIK{padded_cik}.json
//!   3. Optional form_type filter (10-K, 10-Q, 8-K, DEF 14A, ...).
//!
//! Returns metadata + primary document URL — the model is expected to call
//! `web_fetch` on a specific filing URL to read the actual text. Splitting
//! discovery from fetching keeps each tool simple and lets the model decide
//! how deep to drill.
//!
//! SEC requires a real User-Agent including a contact email. We set one that
//! identifies leek; users hitting their own rate limit can override via the
//! LEEK_SEC_USER_AGENT env var.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "sec_filing_fetch";
const DEFAULT_LIMIT: usize = 8;
const MAX_LIMIT: usize = 30;
const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const SUBMISSIONS_URL: &str = "https://data.sec.gov/submissions/CIK{cik}.json";
// SEC requires a UA that "uniquely identifies" the requester and includes a
// real contact email. Their parser is finicky — keep it to the form
// "Name email@domain" without parentheses or version markers, which has
// historically been rejected with 403.
const DEFAULT_USER_AGENT: &str = "Leek Research gradschool.hchen13@gmail.com";
const TICKER_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub struct SecFilingFetchTool {
    http: Client,
    ticker_cache: Mutex<Option<TickerCache>>,
}

struct TickerCache {
    map: HashMap<String, u64>, // upper-case ticker → CIK
    fetched_at: Instant,
}

impl SecFilingFetchTool {
    pub fn new() -> Result<Self> {
        let ua = std::env::var("LEEK_SEC_USER_AGENT").unwrap_or_else(|_| DEFAULT_USER_AGENT.into());
        let http = Client::builder()
            .user_agent(ua)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            ticker_cache: Mutex::new(None),
        })
    }

    async fn ticker_to_cik(&self, ticker: &str, cancel: &CancellationToken) -> Result<u64> {
        let upper = ticker.to_ascii_uppercase();

        // Cache hit?
        if let Ok(guard) = self.ticker_cache.lock() {
            if let Some(cache) = guard.as_ref() {
                if cache.fetched_at.elapsed() < TICKER_CACHE_TTL {
                    if let Some(&cik) = cache.map.get(&upper) {
                        return Ok(cik);
                    }
                    // Cache present but ticker unknown — fall through to refetch
                    // in case the ticker list changed (rare but possible).
                }
            }
        }

        // Fetch fresh ticker→CIK map.
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during ticker map fetch"),
            r = self.http.get(TICKERS_URL).send() => r?,
        };
        let status = resp.status();
        if !status.is_success() {
            bail!("SEC ticker map returned HTTP {}", status.as_u16());
        }
        let raw: serde_json::Value = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during ticker map parse"),
            r = resp.json() => r?,
        };

        // Schema: { "0": {"cik_str": 320193, "ticker": "AAPL", "title": "..."}, ... }
        let mut map: HashMap<String, u64> = HashMap::new();
        if let Some(obj) = raw.as_object() {
            for (_idx, entry) in obj.iter() {
                let Some(t) = entry.get("ticker").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(cik) = entry.get("cik_str").and_then(|v| v.as_u64()) else {
                    continue;
                };
                map.insert(t.to_ascii_uppercase(), cik);
            }
        }
        if let Ok(mut guard) = self.ticker_cache.lock() {
            *guard = Some(TickerCache {
                map: map.clone(),
                fetched_at: Instant::now(),
            });
        }
        map.get(&upper)
            .copied()
            .ok_or_else(|| anyhow!("unknown ticker `{ticker}` (not in SEC company list)"))
    }

    async fn fetch_filings(
        &self,
        cik: u64,
        cancel: &CancellationToken,
    ) -> Result<SubmissionsResponse> {
        let padded = format!("{cik:010}");
        let url = SUBMISSIONS_URL.replace("{cik}", &padded);
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during submissions fetch"),
            r = self.http.get(&url).send() => r?,
        };
        let status = resp.status();
        if !status.is_success() {
            bail!("SEC submissions returned HTTP {}", status.as_u16());
        }
        let parsed: SubmissionsResponse = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during submissions parse"),
            r = resp.json() => r?,
        };
        Ok(parsed)
    }
}

#[derive(Deserialize)]
struct SubmissionsResponse {
    name: Option<String>,
    filings: SubmissionsFilings,
}

#[derive(Deserialize)]
struct SubmissionsFilings {
    recent: RecentFilings,
}

#[derive(Deserialize)]
struct RecentFilings {
    #[serde(rename = "accessionNumber")]
    accession_number: Vec<String>,
    #[serde(rename = "filingDate")]
    filing_date: Vec<String>,
    #[serde(rename = "reportDate", default)]
    report_date: Vec<String>,
    form: Vec<String>,
    #[serde(rename = "primaryDocument")]
    primary_document: Vec<String>,
    #[serde(rename = "primaryDocDescription", default)]
    primary_doc_description: Vec<String>,
}

#[async_trait]
impl ToolHandler for SecFilingFetchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "List recent SEC EDGAR filings for a US-listed company by ticker. \
                 Returns filing metadata (form type, filed date, accession number, \
                 primary document URL). Use this to discover relevant 10-K / 10-Q / \
                 8-K / DEF 14A filings, then call `web_fetch` on a specific URL to \
                 read the actual text. ONLY works for US tickers — for A-shares use \
                 `tushare_quote`, for non-US use web_search."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ticker": {
                        "type": "string",
                        "description": "US ticker symbol (e.g. NVDA, AAPL, BRK.B). Case-insensitive."
                    },
                    "form_type": {
                        "type": "string",
                        "description": "Optional filter, e.g. '10-K', '10-Q', '8-K', 'DEF 14A', 'S-1'. Omit to return all forms."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max filings to return (default 8, max 30). Filings are returned in reverse-chronological order."
                    }
                },
                "required": ["ticker"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let ticker = args
            .get("ticker")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'ticker' argument"))?
            .trim()
            .to_string();
        let form_filter = args.get("form_type").and_then(|v| v.as_str()).map(String::from);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(MAX_LIMIT).max(1))
            .unwrap_or(DEFAULT_LIMIT);

        let cik = self.ticker_to_cik(&ticker, &cancel).await?;
        let submissions = self.fetch_filings(cik, &cancel).await?;

        let r = &submissions.filings.recent;
        let n = r.accession_number.len()
            .min(r.filing_date.len())
            .min(r.form.len())
            .min(r.primary_document.len());

        let mut hits = Vec::new();
        for i in 0..n {
            let form = &r.form[i];
            if let Some(filter) = form_filter.as_deref() {
                if !form.eq_ignore_ascii_case(filter) {
                    continue;
                }
            }
            let acc = &r.accession_number[i];
            let acc_no_dash = acc.replace('-', "");
            let primary = &r.primary_document[i];
            // EDGAR filing index URL — direct link to the primary document.
            let url = format!(
                "https://www.sec.gov/Archives/edgar/data/{cik}/{acc_no_dash}/{primary}"
            );
            let report_date = r.report_date.get(i).cloned().unwrap_or_default();
            let description = r.primary_doc_description.get(i).cloned().unwrap_or_default();

            hits.push(serde_json::json!({
                "form": form,
                "filed": r.filing_date[i],
                "report_period": report_date,
                "accession": acc,
                "primary_doc_url": url,
                "description": description,
            }));
            if hits.len() >= limit {
                break;
            }
        }

        Ok(serde_json::json!({
            "ticker": ticker.to_ascii_uppercase(),
            "company_name": submissions.name,
            "cik": cik,
            "form_filter": form_filter,
            "filings": hits,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_submissions_shape() {
        // Minimal shape sanity — full integration test requires network.
        let raw = serde_json::json!({
            "name": "TEST CORP",
            "filings": {
                "recent": {
                    "accessionNumber": ["0001-23-456"],
                    "filingDate": ["2026-04-01"],
                    "reportDate": ["2026-03-31"],
                    "form": ["10-Q"],
                    "primaryDocument": ["test.htm"],
                    "primaryDocDescription": ["10-Q FILED"]
                }
            }
        });
        let parsed: SubmissionsResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.name.unwrap(), "TEST CORP");
        assert_eq!(parsed.filings.recent.form[0], "10-Q");
    }
}
