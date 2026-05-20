//! `web_fetch` — fetch an HTTP(S) URL and return its body as text.
//!
//! M1's first real, non-deterministic tool. Kept deliberately small (no JS
//! rendering, no crawling) — the milestone's weight is the loop and the
//! guards, not this tool.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;

/// Cap the body so a large page cannot blow up the model's context.
const MAX_BODY_CHARS: usize = 16_000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "web_fetch".into(),
        description: "Fetch the contents of an HTTP(S) URL and return the response \
             body as text. Use it to read a specific web page or HTTP API endpoint \
             that the user named, or that earlier research surfaced. Returns up to \
             ~16000 characters; a longer body is truncated and marked as such. Does \
             not run JavaScript — a page that renders client-side comes back as raw \
             HTML."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute http:// or https:// URL to fetch."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
    }
}

/// UI metadata (REQUIREMENTS §4.1) — registered separately from `spec`;
/// never sent to the model. The result renders as a link-preview card.
pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "读取网页",
        result: ResultArtifact::Card("web_preview"),
        summary: |args| {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let host = reqwest::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| url.to_string());
            format!("读取网页 · {host}")
        },
    }
}

pub async fn run(http: &reqwest::Client, args: &serde_json::Value) -> ToolOutcome {
    let Some(url) = args.get("url").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("web_fetch: missing required string argument 'url'.");
    };
    let url = match validate_url(url) {
        Ok(url) => url,
        Err(e) => return ToolOutcome::error(e),
    };

    let resp = match http.get(url.clone()).timeout(FETCH_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            return ToolOutcome::error(format!("web_fetch: request to '{url}' failed: {e}"));
        }
    };

    let status = resp.status();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
            return ToolOutcome::error(format!(
                "web_fetch: could not read the body of '{url}': {e}"
            ));
        }
    };
    let host = url.host_str().unwrap_or("").to_string();

    if !status.is_success() {
        let preview = truncate_chars(&body, 500).0;
        return ToolOutcome {
            model_output: format!(
                "web_fetch: '{url}' returned HTTP {status}. Body preview: {preview}"
            ),
            display_payload: serde_json::json!({
                "url": url.as_str(), "host": host,
                "status": status.as_u16(), "error": format!("HTTP {status}"),
            }),
            debug_payload: serde_json::json!({
                "url": url.as_str(), "http_status": status.as_u16(),
                "body_chars": body.chars().count(),
            }),
            is_error: true,
        };
    }

    let title = extract_title(&body);
    let (text, truncated) = truncate_chars(&body, MAX_BODY_CHARS);
    let model_output = if truncated {
        format!("{text}\n\n[web_fetch: body truncated at {MAX_BODY_CHARS} characters]")
    } else {
        text
    };
    // The link-preview card reads `display_payload`; the model gets the page
    // text via `model_output`. The two are not parsed from each other.
    ToolOutcome::ok(
        model_output,
        serde_json::json!({
            "url": url.as_str(), "host": host, "title": title,
            "status": status.as_u16(),
            "char_count": body.chars().count(), "truncated": truncated,
        }),
        serde_json::json!({
            "url": url.as_str(), "http_status": status.as_u16(),
            "body_chars": body.chars().count(), "truncated": truncated,
        }),
    )
}

/// Best-effort `<title>` for the link-preview card. ASCII-case folding is
/// enough — only the tag name matters, and lowercasing ASCII leaves byte
/// offsets valid for slicing the original `html`.
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let gt = lower[open..].find('>')? + open + 1;
    let close = lower[gt..].find("</title>")? + gt;
    let title = html
        .get(gt..close)?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        None
    } else {
        Some(title.chars().take(200).collect())
    }
}

fn validate_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| format!("web_fetch: '{raw}' is not a valid absolute URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!(
            "web_fetch: '{raw}' is not fetchable — scheme must be http or https."
        ));
    }

    let host = url
        .host_str()
        .ok_or_else(|| format!("web_fetch: '{raw}' has no host."))?;
    if is_blocked_host(host) {
        return Err(format!(
            "web_fetch: refusing to fetch '{raw}' because local and private network hosts are blocked."
        ));
    }
    Ok(url)
}

fn is_blocked_host(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => is_blocked_ipv4(ip),
        Ok(IpAddr::V6(ip)) => is_blocked_ipv6(ip),
        Err(_) => false,
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
}

/// Take the first `max` characters; report whether anything was dropped.
fn truncate_chars(s: &str, max: usize) -> (String, bool) {
    let mut out = String::new();
    for (count, ch) in s.chars().enumerate() {
        if count >= max {
            return (out, true);
        }
        out.push(ch);
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_url() {
        let http = reqwest::Client::new();
        let out = futures::executor::block_on(run(
            &http,
            &serde_json::json!({ "url": "ftp://example.com" }),
        ));
        assert!(out.is_error);
    }

    #[test]
    fn rejects_local_and_private_hosts() {
        for url in [
            "http://localhost:8964/",
            "http://127.0.0.1:8964/",
            "https://example.com@127.0.0.1/",
            "http://10.0.0.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn accepts_public_http_url() {
        let url = validate_url("https://example.com/path?q=1").unwrap();
        assert_eq!(url.host_str(), Some("example.com"));
    }

    #[test]
    fn truncate_marks_dropped_content() {
        let (text, truncated) = truncate_chars("abcdef", 3);
        assert_eq!(text, "abc");
        assert!(truncated);
        let (text, truncated) = truncate_chars("ab", 3);
        assert_eq!(text, "ab");
        assert!(!truncated);
    }

    #[test]
    fn extract_title_reads_the_head_title() {
        let html = "<html><head><TITLE>  Hello\n  World </TITLE></head><body>x</body>";
        assert_eq!(extract_title(html).as_deref(), Some("Hello World"));
        assert!(extract_title("<html><body>no title</body></html>").is_none());
    }
}
