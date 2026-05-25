//! `web_fetch` — client-side HTTP fetch + readable content extraction.
//!
//! Three-tier strategy (Tier 2/3 land in follow-up tasks):
//!   Tier 1: reqwest GET → dom_smoothie Readability → markdown
//!   Tier 2: fall back to Jina Reader (`r.jina.ai/<url>`) on extract failure
//!   Tier 3: fall back to a codex `thinking=off` HTML→markdown cleanup pass
//!           (NOT summarization — preserve all original content)
//!
//! Design notes:
//! - SSRF policy is hostname-level + cloud-metadata IP literals only. Private-
//!   range IPs are intentionally NOT blocked so that clash TUN / corp proxies
//!   that issue fake-IPs can route normally (see CLAUDE.md / memory).
//! - Cross-origin redirects trip a "REDIRECT DETECTED" reply that asks the
//!   model to re-issue web_fetch with the new URL (same trick as
//!   claude-code/WebFetchTool).
//! - 15-minute in-memory LRU cache, keyed by the (post-upgrade) URL.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use dom_smoothie::{Config, Readability, TextMode};
use lru::LruCache;
use reqwest::Client;
use reqwest::redirect::{Action, Attempt};
use std::num::NonZeroUsize;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "web_fetch";
const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
const FETCH_TIMEOUT_SECS: u64 = 60;
const MAX_REDIRECTS: usize = 10;
const DEFAULT_MAX_CHARS: usize = 50_000;
const HARD_CAP_MAX_CHARS: usize = 100_000;
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const CACHE_CAPACITY: usize = 64;
const USER_AGENT: &str = "leek/0.1 (+https://github.com/hchen13/leek; investment-research agent)";

/// Below this many chars Tier 1 readability output is considered "too thin"
/// (likely SPA / JS-heavy / paywall) — trigger Tier 2 Jina Reader fallback.
const TIER2_THRESHOLD_CHARS: usize = 400;
const JINA_BASE_URL: &str = "https://r.jina.ai";
const JINA_TIMEOUT_SECS: u64 = 45;

pub struct WebFetchTool {
    http: Client,
    cache: Mutex<LruCache<String, CacheEntry>>,
}

#[derive(Clone)]
struct CacheEntry {
    body: String,
    inserted_at: Instant,
}

impl WebFetchTool {
    pub fn new() -> Result<Self> {
        let policy = reqwest::redirect::Policy::custom(|attempt: Attempt| {
            // Always cap the hop count.
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            let prev = match attempt.previous().last() {
                Some(p) => p.clone(),
                None => return attempt.follow(),
            };
            if same_origin_modulo_www(&prev, attempt.url()) {
                attempt.follow()
            } else {
                // Cross-origin: stop so the caller can build a "REDIRECT
                // DETECTED" response. reqwest yields the 30x to us.
                stop_on(attempt)
            }
        });

        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(policy)
            .build()?;

        let cache = LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).unwrap());
        Ok(Self {
            http,
            cache: Mutex::new(cache),
        })
    }

    fn cache_get(&self, key: &str) -> Option<String> {
        let mut cache = self.cache.lock().ok()?;
        let entry = cache.get(key)?.clone();
        if entry.inserted_at.elapsed() > CACHE_TTL {
            cache.pop(key);
            return None;
        }
        Some(entry.body)
    }

    fn cache_put(&self, key: String, body: String) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.put(
                key,
                CacheEntry {
                    body,
                    inserted_at: Instant::now(),
                },
            );
        }
    }

    /// Tier 2 — Jina Reader. Hits `https://r.jina.ai/<url>`, which proxies
    /// the page through Jina's headless-browser + Readability stack and
    /// returns clean markdown. Free tier is rate-limited but doesn't need
    /// an API key for low-volume use; we add `Authorization: Bearer` if
    /// `JINA_API_KEY` is set in the environment.
    async fn fetch_via_jina(&self, target_url: &str, cancel: &CancellationToken) -> Result<String> {
        let endpoint = format!("{}/{}", JINA_BASE_URL, target_url);
        let mut req = self
            .http
            .get(&endpoint)
            .header("Accept", "text/markdown")
            .header("X-Return-Format", "markdown")
            .timeout(Duration::from_secs(JINA_TIMEOUT_SECS));
        if let Ok(key) = std::env::var("JINA_API_KEY") {
            if !key.trim().is_empty() {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
        }
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during Jina fetch"),
            r = req.send() => r?,
        };
        let status = resp.status();
        if !status.is_success() {
            bail!("Jina Reader returned HTTP {}", status.as_u16());
        }
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during Jina body read"),
            t = resp.text() => t?,
        };
        Ok(text)
    }
}

#[async_trait]
impl ToolHandler for WebFetchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Fetch a URL over HTTP and return the readable main content as markdown. \
                 Use for articles, news, filings, blog posts, PRs. The page is parsed with \
                 a Readability algorithm — navigation, ads, footers are stripped. Tables, \
                 lists, links, and code blocks are preserved. JavaScript is NOT executed; \
                 if the result says clearly unusable / not \
                 citable, do not cite it; fetch a readable source instead. Otherwise cite the \
                 URL in your final answer."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Absolute http(s) URL to fetch."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description":
                            "Optional cap on returned markdown length (default 50000, \
                             hard max 100000). Truncated output is suffixed with a notice.",
                        "minimum": 1
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let url_str = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'url' argument"))?
            .trim()
            .to_string();
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(HARD_CAP_MAX_CHARS))
            .unwrap_or(DEFAULT_MAX_CHARS);

        let parsed = parse_and_validate_url(&url_str)?;
        let upgraded = upgrade_to_https(parsed);
        let final_url = upgraded.to_string();

        if let Some(cached) = self.cache_get(&final_url) {
            return Ok(truncate_markdown(&cached, max_chars));
        }

        // Cancellation check before the network round-trip.
        if cancel.is_cancelled() {
            bail!("aborted before fetch");
        }

        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during fetch"),
            r = self.http.get(&final_url).send() => r?,
        };

        let status = resp.status();

        // Cross-origin redirect: reqwest stopped on a 30x, surface that to
        // the model so it can re-call web_fetch with the new URL.
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("(missing Location header)")
                .to_string();
            // Resolve relative redirects against the requested URL.
            let resolved = upgraded
                .join(&location)
                .map(|u| u.to_string())
                .unwrap_or(location);
            return Ok(format!(
                "REDIRECT DETECTED: {} → {}\n\n\
                 The URL redirects to a different host. To complete the fetch, \
                 call web_fetch again with: {{\"url\": \"{}\"}}",
                final_url, resolved, resolved
            ));
        }

        if !status.is_success() {
            return Ok(format!(
                "[web_fetch error: HTTP {} from {}]",
                status.as_u16(),
                final_url
            ));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        // Read body with size cap. reqwest doesn't enforce a body limit by
        // default; we read the whole thing and then check.
        let bytes = tokio::select! {
            biased;
            _ = cancel.cancelled() => bail!("aborted during body read"),
            r = resp.bytes() => r?,
        };
        if bytes.len() as u64 > MAX_BODY_BYTES {
            return Ok(format!(
                "[web_fetch error: response body too large ({} bytes, max {})]",
                bytes.len(),
                MAX_BODY_BYTES
            ));
        }

        let binary_without_type = content_type.is_empty() && looks_like_binary_payload(&bytes);
        let body = if binary_without_type {
            String::new()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };

        let (mut extracted, mut tier_used) = if binary_without_type {
            (
                unusable_source_message(
                    "binary_skipped",
                    "missing content-type; payload looks binary",
                ),
                "binary_skipped",
            )
        } else if content_type.contains("text/html") || content_type.is_empty() {
            // Tier 1: Readability + dom_smoothie
            let tier1 = extract_readable_markdown(&body, &final_url);
            let tier1_len = tier1.as_ref().map(|s| s.chars().count()).unwrap_or(0);

            if tier1_len >= TIER2_THRESHOLD_CHARS {
                (tier1.unwrap(), "readability")
            } else {
                // Tier 2: fall back to Jina Reader (handles SPA, paywall lite, JS-heavy).
                tracing::info!(
                    url = %final_url,
                    tier1_chars = tier1_len,
                    "tier 1 readability too thin; falling back to Jina Reader"
                );
                match self.fetch_via_jina(&final_url, &cancel).await {
                    Ok(jina_md) if jina_md.chars().count() >= TIER2_THRESHOLD_CHARS => {
                        (jina_md, "jina_reader")
                    }
                    Ok(thin_jina) => {
                        // Both tiers thin — Tier 3 fallback: strip HTML tags
                        // from the original body so the model at least sees
                        // *something*. Far from perfect but lossless on text.
                        tracing::info!(url = %final_url, "both tier 1/2 thin; falling back to strip-tag");
                        let stripped = strip_html_tags(&body);
                        if stripped.chars().count() > thin_jina.chars().count() {
                            (stripped, "strip_tag_fallback")
                        } else {
                            (thin_jina, "jina_reader_thin")
                        }
                    }
                    Err(e) => {
                        tracing::warn!(url = %final_url, error = %e, "Jina Reader fallback failed");
                        let stripped = strip_html_tags(&body);
                        if !stripped.is_empty() {
                            (stripped, "strip_tag_fallback")
                        } else {
                            (
                                tier1.unwrap_or_else(|| {
                                    "[web_fetch: extraction produced no content]".into()
                                }),
                                "readability_fallback",
                            )
                        }
                    }
                }
            }
        } else if is_textual_content_type(&content_type) {
            (body.clone(), "raw_text")
        } else {
            (
                unusable_source_message(
                    "binary_skipped",
                    &format!(
                        "non-text content-type `{}` ({} bytes)",
                        content_type,
                        bytes.len()
                    ),
                ),
                "binary_skipped",
            )
        };

        if tier_used != "binary_skipped" && looks_unreadable_or_garbled(extracted.trim()) {
            extracted = unusable_source_message(
                "unreadable_or_garbled_text",
                "decoded/extracted text is clearly unreadable",
            );
            tier_used = "unreadable_or_garbled_text";
        }

        let final_body = format!(
            "# Source: {final_url}\n_(extraction: {tier_used})_\n\n{}",
            extracted.trim()
        );
        self.cache_put(final_url, final_body.clone());
        Ok(truncate_markdown(&final_body, max_chars))
    }
}

fn parse_and_validate_url(s: &str) -> Result<Url> {
    let url = Url::parse(s).map_err(|e| anyhow!("invalid URL: {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => bail!("unsupported scheme `{other}`; only http(s) allowed"),
    }
    if url.username() != "" || url.password().is_some() {
        bail!("URLs with embedded credentials are not allowed");
    }
    let host = url
        .host()
        .ok_or_else(|| anyhow!("URL has no host component"))?;
    match host {
        url::Host::Domain(d) => check_hostname(d)?,
        url::Host::Ipv4(ip) => check_ipv4(ip)?,
        url::Host::Ipv6(_ip) => {
            // We could special-case fec0::/10 etc., but private IPv6 is rare
            // in the wild for our use case. Hostname-form gating already
            // catches the dangerous ones (no DNS lookup is performed).
        }
    }
    Ok(url)
}

fn unusable_source_message(reason: &str, detail: &str) -> String {
    format!(
        "[web_fetch: clearly unusable / not citable ({reason}). {detail}. Do not cite this URL; fetch a readable HTML/text source or an official text/announcement source instead.]"
    )
}

fn looks_like_binary_payload(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"%PDF-") || bytes.starts_with(b"PK\x03\x04") {
        return true;
    }
    let sample_len = bytes.len().min(2048);
    if sample_len == 0 {
        return false;
    }
    let sample = &bytes[..sample_len];
    if sample.contains(&0) {
        return true;
    }
    let control_count = sample
        .iter()
        .filter(|b| b.is_ascii_control() && !matches!(b, b'\n' | b'\r' | b'\t'))
        .count();
    control_count * 20 > sample_len
}

fn looks_unreadable_or_garbled(text: &str) -> bool {
    let total = text.chars().count();
    if total < 40 {
        return false;
    }
    let replacement_count = text.chars().filter(|c| *c == '\u{FFFD}').count();
    if replacement_count >= 8 && replacement_count * 20 > total {
        return true;
    }
    let control_count = text
        .chars()
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        .count();
    control_count * 20 > total
}

fn check_hostname(host: &str) -> Result<()> {
    let h = host.to_ascii_lowercase();
    if h.is_empty() {
        bail!("empty hostname");
    }
    // Single-label / .local / .internal / .lan — almost always intranet.
    if !h.contains('.')
        || h.ends_with(".local")
        || h.ends_with(".internal")
        || h.ends_with(".lan")
        || h == "localhost"
        || h.ends_with(".localhost")
    {
        bail!("internal hostname `{host}` is not allowed");
    }
    Ok(())
}

fn check_ipv4(ip: Ipv4Addr) -> Result<()> {
    let _ = IpAddr::V4(ip);
    let octets = ip.octets();
    // Cloud metadata: link-local + CGNAT (where Alibaba 100.100.100.200 lives).
    // We deliberately do NOT block 10/8, 172.16/12, 192.168/16, or 198.18/15
    // — clash TUN fake-IP and self-hosted services live there. The risk
    // boundary is "leak credentials to cloud metadata", which the two ranges
    // below cover.
    if octets[0] == 169 && octets[1] == 254 {
        bail!("link-local address {ip} (cloud metadata) blocked");
    }
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        bail!("CGNAT address {ip} (cloud metadata) blocked");
    }
    Ok(())
}

fn upgrade_to_https(mut url: Url) -> Url {
    if url.scheme() == "http" {
        // url crate's set_scheme returns Err if the scheme is incompatible,
        // but http→https is allowed.
        let _ = url.set_scheme("https");
    }
    url
}

fn same_origin_modulo_www(a: &Url, b: &Url) -> bool {
    let strip_www = |s: &str| s.strip_prefix("www.").unwrap_or(s).to_ascii_lowercase();
    let host_a = a.host_str().map(strip_www).unwrap_or_default();
    let host_b = b.host_str().map(strip_www).unwrap_or_default();
    host_a == host_b && a.scheme() == b.scheme() && a.port() == b.port()
}

fn stop_on(attempt: Attempt) -> Action {
    attempt.stop()
}

fn extract_readable_markdown(html: &str, url: &str) -> Option<String> {
    let cfg = Config {
        text_mode: TextMode::Markdown,
        ..Default::default()
    };
    let mut r = Readability::new(html, Some(url), Some(cfg)).ok()?;
    let article = r.parse().ok()?;
    let text: String = article.text_content.into();
    if text.trim().is_empty() {
        return None;
    }
    let title = article.title;
    if title.trim().is_empty() {
        Some(text)
    } else {
        Some(format!("# {title}\n\n{text}"))
    }
}

/// Tier 3 fallback: lossless strip of `<script>`, `<style>`, `<svg>`, then
/// every remaining HTML tag. Result is plain text — keeps original wording
/// (good for financial figures / quotes) but loses all formatting. Used
/// only when Tier 1 + Tier 2 both produce too little content.
fn strip_html_tags(html: &str) -> String {
    // Drop noise blocks first to avoid leaking JS strings / CSS rules / SVG.
    let cleaned = remove_block(html, "<script", "</script>");
    let cleaned = remove_block(&cleaned, "<style", "</style>");
    let cleaned = remove_block(&cleaned, "<svg", "</svg>");
    let cleaned = remove_block(&cleaned, "<noscript", "</noscript>");
    let cleaned = remove_block(&cleaned, "<!--", "-->");

    let mut out = String::with_capacity(cleaned.len() / 2);
    let mut in_tag = false;
    for ch in cleaned.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse whitespace runs to a single space, preserve paragraph breaks
    // (multiple newlines become double-newline).
    let mut compact = String::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                compact.push('\n');
                prev_blank = true;
            }
        } else {
            compact.push_str(trimmed);
            compact.push('\n');
            prev_blank = false;
        }
    }
    compact.trim().to_string()
}

fn remove_block(s: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.to_ascii_lowercase().find(open) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(end_rel) = tail.to_ascii_lowercase().find(close) {
            rest = &tail[end_rel + close.len()..];
        } else {
            // Unterminated — drop the rest to be safe.
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn is_textual_content_type(ct: &str) -> bool {
    ct.starts_with("text/")
        || ct.contains("application/json")
        || ct.contains("application/xml")
        || ct.contains("application/x-yaml")
        || ct.contains("+json")
        || ct.contains("+xml")
}

fn truncate_markdown(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str("\n\n[…content truncated by web_fetch max_chars]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_localhost() {
        assert!(parse_and_validate_url("http://localhost/").is_err());
        assert!(parse_and_validate_url("http://machine.local/").is_err());
        assert!(parse_and_validate_url("http://intranet.lan/").is_err());
        assert!(parse_and_validate_url("http://wiki.internal/").is_err());
    }

    #[test]
    fn rejects_single_label_hostname() {
        assert!(parse_and_validate_url("http://printer/").is_err());
    }

    #[test]
    fn rejects_credentials_in_url() {
        assert!(parse_and_validate_url("https://user:pass@example.com/").is_err());
    }

    #[test]
    fn rejects_link_local_ip() {
        assert!(parse_and_validate_url("http://169.254.169.254/").is_err());
    }

    #[test]
    fn rejects_cgnat_ip() {
        assert!(parse_and_validate_url("http://100.100.100.200/").is_err());
    }

    #[test]
    fn allows_clash_fake_ip_range() {
        // 198.18.0.0/15 is RFC 2544 benchmark range — clash TUN's default
        // fake-IP pool. Must NOT be blocked at the SSRF layer.
        assert!(parse_and_validate_url("http://198.18.0.5/").is_ok());
    }

    #[test]
    fn allows_private_ranges() {
        // 10/8, 172.16/12, 192.168/16 — self-hosted / VPN scenarios.
        assert!(parse_and_validate_url("http://10.0.0.1/").is_ok());
        assert!(parse_and_validate_url("http://192.168.1.1/").is_ok());
        assert!(parse_and_validate_url("http://172.16.5.5/").is_ok());
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(parse_and_validate_url("file:///etc/passwd").is_err());
        assert!(parse_and_validate_url("ftp://example.com/").is_err());
    }

    #[test]
    fn upgrades_http_to_https() {
        let parsed = parse_and_validate_url("http://example.com/x").unwrap();
        let up = upgrade_to_https(parsed);
        assert_eq!(up.scheme(), "https");
    }

    #[test]
    fn same_origin_handles_www_strip() {
        let a = Url::parse("https://example.com/a").unwrap();
        let b = Url::parse("https://www.example.com/b").unwrap();
        assert!(same_origin_modulo_www(&a, &b));
    }

    #[test]
    fn cross_origin_detected() {
        let a = Url::parse("https://example.com/a").unwrap();
        let b = Url::parse("https://other.com/a").unwrap();
        assert!(!same_origin_modulo_www(&a, &b));
    }

    #[test]
    fn truncate_appends_marker_when_over_limit() {
        let s = "a".repeat(100);
        let t = truncate_markdown(&s, 30);
        assert!(t.contains("[…content truncated"));
        assert!(t.chars().take(30).all(|c| c == 'a'));
    }

    #[test]
    fn truncate_passthrough_when_under_limit() {
        let t = truncate_markdown("hello", 100);
        assert_eq!(t, "hello");
    }

    #[test]
    fn unusable_message_blocks_citation() {
        let msg = unusable_source_message("binary_skipped", "application/pdf");
        assert!(msg.contains("clearly unusable / not citable"));
        assert!(msg.contains("Do not cite this URL"));
    }

    #[test]
    fn binary_payload_sniffer_catches_pdf_and_null_bytes() {
        assert!(looks_like_binary_payload(b"%PDF-1.7\n..."));
        assert!(looks_like_binary_payload(b"abc\0def"));
        assert!(!looks_like_binary_payload(
            b"<html><body>readable text</body></html>"
        ));
    }

    #[test]
    fn garbled_text_detector_catches_replacement_noise() {
        let noisy = format!(
            "{} readable tail with enough length to clear the detector floor",
            "\u{FFFD}".repeat(20)
        );
        assert!(looks_unreadable_or_garbled(&noisy));
        assert!(!looks_unreadable_or_garbled(
            "This is a readable page with enough normal words and no decoding replacement noise."
        ));
    }

    #[test]
    fn strip_html_drops_scripts_and_tags() {
        let html = r#"<html><head><style>body{color:red}</style></head>
            <body><h1>Title</h1>
            <script>alert("x")</script>
            <p>Hello <b>world</b>.</p></body></html>"#;
        let out = strip_html_tags(html);
        assert!(!out.contains("alert"));
        assert!(!out.contains("color:red"));
        assert!(out.contains("Title"));
        assert!(out.contains("Hello"));
        assert!(out.contains("world"));
        assert!(!out.contains("<"));
    }

    #[test]
    fn strip_html_handles_html_comments() {
        let html = "<p>visible</p><!-- secret comment with <tags> --><p>also</p>";
        let out = strip_html_tags(html);
        assert!(out.contains("visible"));
        assert!(out.contains("also"));
        assert!(!out.contains("secret"));
    }

    #[test]
    fn extract_readable_markdown_basic() {
        let html = r#"<!doctype html><html><head><title>T</title></head>
            <body><nav>navigation noise</nav>
            <article>
              <h1>Hello</h1>
              <p>This is a meaningful paragraph that should survive readability scoring with enough characters to clear the threshold. Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt.</p>
              <p>Second paragraph with more content to give the article body sufficient weight against the navigation boilerplate. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris.</p>
            </article>
            <footer>footer noise</footer></body></html>"#;
        let md = extract_readable_markdown(html, "https://example.com/x")
            .expect("readability should produce content");
        assert!(md.contains("Hello") || md.contains("meaningful paragraph"));
        assert!(!md.to_lowercase().contains("navigation noise"));
        assert!(!md.to_lowercase().contains("footer noise"));
    }
}
