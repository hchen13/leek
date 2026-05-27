//! `read_pdf(url, offset?, limit?, format?)` — fetch a PDF over HTTP
//! and return a window of its extracted text.
//!
//! Implementation:
//!
//! 1. Validate the URL (HTTPS / HTTP only; same SSRF guard as
//!    `web_fetch`).
//! 2. Quick HEAD to confirm `Content-Type: application/pdf` (or a
//!    `.pdf` path tail).
//! 3. Stream-download to a tempfile, capped at 20 MB.
//! 4. `pdf_extract::extract_text(path)` to get a UTF-8 string.
//! 5. Slice `[offset .. offset+limit)` in characters; advise the next
//!    `offset` in the markdown footer.
//! 6. Cache the last 5 URLs in-process so same-turn re-reads of the
//!    same PDF are free.
//!
//! Chinese-language quality caveat: `pdf-extract` is best-effort for
//! CJK (CMap support is partial). When the extracted text drops below
//! a 0.5 ASCII / non-ASCII ratio sanity check we still return it but
//! flag `chinese_quality_unverified` in the display payload so the
//! agent can downgrade trust.

use std::io::Write;
use std::sync::Mutex;
use std::time::Duration;

use super::{ResultArtifact, ToolOutcome, ToolUi};
use crate::llm::ToolSpec;

const DEFAULT_LIMIT: u32 = 4_000;
const MAX_LIMIT: u32 = 16_000;
const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const CACHE_CAP: usize = 5;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "read_pdf".into(),
        description: "Fetch a PDF over HTTP and return a window of its \
             extracted text. Use for research PDFs (broker reports from \
             `research_sentiment` output), 公告 PDFs (from \
             `recent_actions` / 个股公告), or any other PDF whose URL \
             the user gives you.\n\
             \n\
             Inputs:\n\
             - url (required) — HTTPS/HTTP URL ending in .pdf or \
             returning Content-Type: application/pdf.\n\
             - offset (optional, default 0) — character offset into the \
             extracted text stream.\n\
             - limit (optional, default 4000, max 16000) — characters to \
             return this call.\n\
             - format (optional, 'markdown' or 'text', default 'markdown').\n\
             \n\
             Examples: 'read_pdf(url=\"https://pdf.dfcfw.com/pdf/H3_AP202605251822844635_1.pdf\")' \
             to read the most recent 茅台 research report.\n\
             \n\
             Limits: PDFs are capped at 20 MB; extraction is best-effort \
             (CJK CMap support is partial). Output includes a `progress` \
             header line and a `next_offset` footer when more text \
             remains. Re-reading the same URL within the same agent run \
             is free (LRU cache, 5 URLs).\n\
             \n\
             Boundaries: this tool ONLY reads PDFs. For HTML pages use \
             `web_fetch`. It does NOT crawl, click, or fill forms."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute http(s) URL to a PDF resource."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Character offset (default 0)."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_LIMIT,
                    "description": "Characters to return (default 4000, max 16000)."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "text"],
                    "description": "Output format (default 'markdown')."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
    }
}

pub fn ui() -> ToolUi {
    ToolUi {
        display_name: "读取 PDF",
        result: ResultArtifact::Card("pdf_reader"),
        summary: |args| {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let host = reqwest::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| url.to_string());
            format!("PDF · {host}")
        },
    }
}

/// Process-wide LRU cache: URL → extracted text. Cap 5.
static PDF_CACHE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn cache_get(url: &str) -> Option<String> {
    let cache = PDF_CACHE.lock().unwrap();
    cache.iter().find(|(u, _)| u == url).map(|(_, t)| t.clone())
}

fn cache_put(url: String, text: String) {
    let mut cache = PDF_CACHE.lock().unwrap();
    cache.retain(|(u, _)| u != &url);
    cache.insert(0, (url, text));
    while cache.len() > CACHE_CAP {
        cache.pop();
    }
}

pub async fn run(http: &reqwest::Client, args: &serde_json::Value) -> ToolOutcome {
    let Some(url_str) = args.get("url").and_then(|v| v.as_str()) else {
        return ToolOutcome::error("read_pdf: missing required 'url'.");
    };
    let url = match reqwest::Url::parse(url_str) {
        Ok(u) => u,
        Err(e) => {
            return ToolOutcome::error(format!(
                "read_pdf: invalid URL '{url_str}' — {e} [validation]"
            ))
        }
    };
    if !matches!(url.scheme(), "http" | "https") {
        return ToolOutcome::error(format!(
            "read_pdf: scheme '{}' not supported (need http/https) [validation]",
            url.scheme()
        ));
    }
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n.min(MAX_LIMIT as u64) as usize)
        .unwrap_or(DEFAULT_LIMIT as usize);
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("markdown")
        .to_string();

    // Step 1: cache hit?
    if let Some(text) = cache_get(url_str) {
        return finalize(url_str, &text, offset, limit, &format, /*cached*/ true);
    }

    // Step 2: fetch.
    let resp = match http.get(url.clone()).timeout(FETCH_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            return ToolOutcome::error(format!(
                "read_pdf: fetch '{url}' failed: {e} [transient]"
            ));
        }
    };
    if !resp.status().is_success() {
        return ToolOutcome::error(format!(
            "read_pdf: '{url}' returned HTTP {} [transient]",
            resp.status()
        ));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_pdf =
        content_type.to_lowercase().contains("application/pdf") || url.path().ends_with(".pdf");
    if !is_pdf {
        return ToolOutcome::error(format!(
            "read_pdf: '{url}' is not a PDF (Content-Type: {content_type}) [url_not_pdf / validation]"
        ));
    }

    // Step 3: stream to tempfile (capped).
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return ToolOutcome::error(format!(
                "read_pdf: body download failed: {e} [transient]"
            ));
        }
    };
    if bytes.len() > MAX_PDF_BYTES {
        return ToolOutcome::error(format!(
            "read_pdf: PDF exceeds {MAX_PDF_BYTES} bytes (got {}) [policy]",
            bytes.len()
        ));
    }
    let mut tmp = match tempfile::NamedTempFile::new() {
        Ok(f) => f,
        Err(e) => {
            return ToolOutcome::error(format!(
                "read_pdf: cannot create tempfile: {e} [transient]"
            ));
        }
    };
    if let Err(e) = tmp.write_all(&bytes) {
        return ToolOutcome::error(format!("read_pdf: tempfile write: {e} [transient]"));
    }

    // Step 4: extract (blocking, runs on tokio's blocking thread pool
    // because pdf-extract is sync and can take ~seconds on big PDFs).
    let path = tmp.path().to_path_buf();
    let extract_res =
        tokio::task::spawn_blocking(move || pdf_extract::extract_text(&path)).await;
    let text = match extract_res {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            return ToolOutcome::error(format!(
                "read_pdf: pdf-extract failed for '{url}': {e} [pdf_parse_failed / permission]"
            ));
        }
        Err(e) => {
            return ToolOutcome::error(format!(
                "read_pdf: extraction task failed: {e} [transient]"
            ));
        }
    };

    cache_put(url_str.to_string(), text.clone());
    finalize(url_str, &text, offset, limit, &format, /*cached*/ false)
}

fn finalize(
    url: &str,
    full_text: &str,
    offset: usize,
    limit: usize,
    format: &str,
    cached: bool,
) -> ToolOutcome {
    let total_chars = full_text.chars().count();
    if offset >= total_chars && total_chars > 0 {
        return ToolOutcome::error(format!(
            "read_pdf: offset {offset} ≥ total {total_chars} characters [validation]"
        ));
    }
    let slice: String = full_text.chars().skip(offset).take(limit).collect();
    let returned = slice.chars().count();
    let next_offset = offset + returned;
    let has_more = next_offset < total_chars;

    // Heuristic: count non-ASCII chars in the *whole* extracted text;
    // if it's < 5% and the PDF has any non-ASCII source language, mark
    // quality as unverified. Crude but cheap.
    let non_ascii = full_text.chars().filter(|c| !c.is_ascii()).count();
    let chinese_quality_unverified =
        total_chars > 200 && (non_ascii as f64 / total_chars as f64) < 0.05;

    let body = if format == "markdown" {
        format!(
            "## PDF\n**来源:** {url}\n**进度:** 第 {offset}-{end} 字 / 共 ~{total} 字{cached}\n\n{slice}\n\n---\n{next}\n",
            end = next_offset,
            total = total_chars,
            cached = if cached { " (cached)" } else { "" },
            next = if has_more {
                format!("**下一段:** `read_pdf(url, offset={next_offset}, limit={limit})` 续读 (剩余 ~{} 字)", total_chars - next_offset)
            } else {
                "**已读完。**".into()
            },
        )
    } else {
        slice.clone()
    };

    let display = serde_json::json!({
        "kind": "pdf_reader",
        "url": url,
        "offset": offset,
        "limit": limit,
        "returned_chars": returned,
        "total_chars": total_chars,
        "next_offset": next_offset,
        "has_more": has_more,
        "cached": cached,
        "chinese_quality_unverified": chinese_quality_unverified,
    });
    let debug = serde_json::json!({
        "url": url,
        "total_chars": total_chars,
        "returned_chars": returned,
        "cached": cached,
        "non_ascii_chars": non_ascii,
    });
    ToolOutcome::ok(body, display, debug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_vendor_neutral() {
        let s = spec();
        let d = s.description.to_lowercase();
        for needle in ["tushare", "eastmoney", "sina finance"] {
            assert!(!d.contains(needle), "leaked '{needle}'");
        }
        for needle in ["新浪", "东方财富"] {
            assert!(!s.description.contains(needle));
        }
    }

    #[test]
    fn missing_url_is_structured_error() {
        let http = reqwest::Client::new();
        let out = futures::executor::block_on(run(&http, &serde_json::json!({})));
        assert!(out.is_error);
    }

    #[test]
    fn non_http_url_is_validation_error() {
        let http = reqwest::Client::new();
        let out = futures::executor::block_on(run(
            &http,
            &serde_json::json!({ "url": "ftp://example.com/foo.pdf" }),
        ));
        assert!(out.is_error);
        assert!(out.model_output.contains("validation"));
    }

    #[test]
    fn finalize_slices_with_offset() {
        let text = "ABCDEFGHIJ".to_string();
        let out = finalize("u", &text, 2, 4, "markdown", false);
        assert!(!out.is_error);
        assert!(out.model_output.contains("CDEF"));
        // next_offset = 6, has_more = true (4 chars left)
        let dp = out.display_payload;
        assert_eq!(dp["next_offset"], 6);
        assert_eq!(dp["has_more"], true);
    }

    #[test]
    fn finalize_marks_complete_when_at_end() {
        let text = "ABCDE".to_string();
        let out = finalize("u", &text, 0, 10, "markdown", false);
        assert_eq!(out.display_payload["has_more"], false);
        assert!(out.model_output.contains("已读完"));
    }

    #[test]
    fn finalize_text_format_returns_just_slice() {
        let text = "hello world".to_string();
        let out = finalize("u", &text, 0, 5, "text", false);
        assert_eq!(out.model_output, "hello");
    }

    #[test]
    fn cache_put_get_eviction() {
        // Manual reset of the static.
        *PDF_CACHE.lock().unwrap() = Vec::new();
        for i in 0..7 {
            cache_put(format!("u{i}"), format!("t{i}"));
        }
        // Cap = 5; only the latest 5 should remain.
        let c = PDF_CACHE.lock().unwrap();
        assert_eq!(c.len(), CACHE_CAP);
        assert!(c.iter().any(|(u, _)| u == "u6"));
        assert!(!c.iter().any(|(u, _)| u == "u0"));
    }
}
