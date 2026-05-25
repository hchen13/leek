//! `CodexClient` — the concrete client for the codex backend.
//!
//! One client, no trait (ARCHITECTURE §2). It owns token lifecycle (load
//! from the vault, refresh near expiry) and turns a `ChatRequest` into a
//! streamed `LlmEvent` flow over the OpenAI Responses API.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use super::oauth::{self, CodexTokens};
use super::{responses, ChatRequest, LlmEvent};
use crate::vault::{auth_tokens, llm_transcripts};

/// codex backend Responses endpoint.
const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
/// Refresh the access token once it is within this many seconds of expiry.
const REFRESH_SKEW_SECS: i64 = 120;
/// Cap on the HTTP-error body excerpt the client surfaces to the fatal-reason
/// classifier. Long enough to carry a useful codex error message, short enough
/// that a runaway HTML 503 page doesn't bloat the assistant_done payload or
/// the `turn_metrics.fatal_error` column.
const MAX_ERROR_BODY_CHARS: usize = 200;

/// M3.3: typed failure mode for the initial codex POST (or the connect /
/// TLS / DNS step that precedes it). Surfaced as an `anyhow::Error` so the
/// existing error plumbing keeps working — the agent loop downcasts to this
/// enum to classify the failure into a user-facing `FatalReason`.
#[derive(Debug, Clone)]
pub enum CodexCallError {
    /// codex replied with a non-2xx status; the excerpt is the (redacted,
    /// truncated) response body. `status` distinguishes 4xx (caller fault —
    /// auth / oversize prompt) from 5xx (codex side trouble — usually a
    /// retry candidate).
    Http { status: u16, body_excerpt: String },
    /// The HTTP request never produced a response — DNS lookup failed, TCP
    /// connect refused, TLS handshake errored, connection reset, etc. The
    /// `detail` is the reqwest error chain, redacted of Bearer tokens.
    Connection { detail: String },
}

impl std::fmt::Display for CodexCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodexCallError::Http { status, body_excerpt } => {
                if body_excerpt.is_empty() {
                    write!(f, "codex backend returned HTTP {status}")
                } else {
                    write!(f, "codex backend returned HTTP {status}: {body_excerpt}")
                }
            }
            CodexCallError::Connection { detail } => {
                write!(f, "could not reach codex backend: {detail}")
            }
        }
    }
}

impl std::error::Error for CodexCallError {}

/// The concrete codex client. Cheap to `clone` — everything inside is shared.
#[derive(Clone)]
pub struct CodexClient {
    pool: SqlitePool,
    user_id: String,
    http: reqwest::Client,
    /// In-memory copy of the active tokens, so a turn does not hit the vault
    /// on every iteration.
    cached: Arc<Mutex<Option<CodexTokens>>>,
}

/// Snapshot of stored token state, for `leek auth status`.
pub struct TokenStatus {
    pub account_id: Option<String>,
    pub expires_at: String,
    pub expired: bool,
    pub updated_at: String,
}

impl CodexClient {
    pub fn new(pool: SqlitePool, user_id: impl Into<String>) -> Result<Self> {
        // M3.4: tighten reqwest-level timeouts so a wedged connection cannot
        // outlive the SSE-side idle detector (M3.3) — and, more importantly,
        // so an iter that never streams a single byte cannot hang for the
        // codex backend's own retry budget (~5 min 11s observed live).
        //
        //   connect_timeout (30s):  TCP/TLS handshake cap. The pre-M3.4
        //                           20s value was fine; bumped to 30s for
        //                           a small DNS-flake margin while still
        //                           well below the retry's first backoff.
        //   timeout (300s):         Overall request lifetime cap. xhigh
        //                           reasoning + builtin search legitimately
        //                           runs minutes; 5 min per *iter* is the
        //                           empirical worst case (longest observed
        //                           xhigh turn was 5m41s across 7 iters,
        //                           so every iter is well under 5min). This
        //                           is a hard floor — if codex's own backend
        //                           wedges, reqwest aborts before the user
        //                           waits longer than this.
        //   pool_idle_timeout(60s): Slightly bumped from 15s so codex's
        //                           keep-alive connections survive the
        //                           tool-dispatch gap between iters.
        //
        // The retry wrapper (see `llm::retry`) sits on top: a reqwest timeout
        // surfaces as a `CodexCallError::Connection`, the wrapper classifies
        // it retryable, and the user typically never notices the failure.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(300))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .context("building HTTP client for codex")?;
        Ok(Self {
            pool,
            user_id: user_id.into(),
            http,
            cached: Arc::new(Mutex::new(None)),
        })
    }

    /// Send one chat request; returns the streamed, normalized event flow.
    ///
    /// Hooks F2 transcript archival around the request: the raw request
    /// body is inserted before the POST (so a mid-stream crash still
    /// leaves evidence), and the raw SSE response is finalized when the
    /// returned stream ends — natural end, consumer-side drop, or a
    /// non-2xx that never streamed. The hooks are best-effort; their
    /// failures log and don't break the call (the model interaction
    /// matters more than the debug log).
    pub async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let access_token = self.access_token().await?;
        let body = responses::build_request_body(&req);
        let started_at = chrono::Utc::now().to_rfc3339();

        // F2: archive the request body before sending it.
        let request_bytes = serde_json::to_vec(&body)
            .context("serializing request body for the transcript archive")?;
        let row_id = match llm_transcripts::insert_request(
            &self.pool,
            &req.session_id,
            &req.turn_id,
            req.iteration,
            &req.model,
            &request_bytes,
            &started_at,
        )
        .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    session_id = %req.session_id,
                    turn_id = %req.turn_id,
                    iteration = req.iteration,
                    "inserting llm_transcripts request row failed; \
                     proceeding without archive for this call",
                );
                None
            }
        };

        let resp = match self
            .http
            .post(RESPONSES_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // M3.3: surface a typed `CodexCallError::Connection` so the
                // agent loop can classify the failure as connection_failed
                // and render an actionable hint card. `redact()` scrubs the
                // Bearer token in case reqwest's error chain echoes it
                // (defensive — it doesn't today).
                let detail = redact(&format!("{e:#}"));
                return Err(anyhow::Error::new(CodexCallError::Connection { detail }));
            }
        };

        let status = resp.status();
        if !status.is_success() {
            // F2: archive the error body verbatim, then bail with a typed
            // CodexCallError::Http carrying a redacted, truncated excerpt
            // (the body excerpt lands in `turn_metrics.fatal_error` and on
            // the `assistant_done` event, so it must not leak tokens /
            // grow unbounded).
            let text = resp.text().await.unwrap_or_default();
            if let Some(row_id) = row_id {
                let now = chrono::Utc::now().to_rfc3339();
                if let Err(e) = llm_transcripts::finalize(
                    &self.pool,
                    row_id,
                    text.as_bytes(),
                    status.as_u16() as i64,
                    &now,
                )
                .await
                {
                    tracing::warn!(error = %e, row_id, "finalizing failed-request transcript");
                }
            }
            let body_excerpt = truncate_chars(&redact(&text), MAX_ERROR_BODY_CHARS);
            return Err(anyhow::Error::new(CodexCallError::Http {
                status: status.as_u16(),
                body_excerpt,
            }));
        }

        // F2: tap each chunk into a shared buffer; the buffer accumulates
        // the raw SSE response, which finalize writes back to the vault
        // when the stream terminates (natural end or consumer-side drop).
        let raw_bytes = resp.bytes_stream();
        let buffer = Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
        let completed = Arc::new(AtomicBool::new(false));

        let buffer_for_tap = buffer.clone();
        let tapped = async_stream::stream! {
            futures::pin_mut!(raw_bytes);
            while let Some(chunk) = raw_bytes.next().await {
                if let Ok(ref b) = chunk {
                    buffer_for_tap.lock().await.extend_from_slice(b);
                }
                yield chunk;
            }
        };

        let parsed = responses::parse_sse_stream(tapped);

        // RAII guard: when the wrapper stream below is dropped — either
        // because it ran to the end or because the consumer cancelled
        // (an idle_timeout cuts the agent loop short) — spawn a task to
        // finalize the row with whatever bytes accumulated. `completed`
        // distinguishes "stream ran to the end" (status=200) from "stream
        // was cut short" (status=0, sentinel for incomplete).
        let guard = row_id.map(|id| TranscriptGuard {
            pool: self.pool.clone(),
            row_id: id,
            buffer: buffer.clone(),
            completed: completed.clone(),
        });

        let final_stream = async_stream::stream! {
            // Hold the guard inside the stream so its drop tracks the
            // stream's own drop — `move` would defeat that.
            let _g = guard;
            futures::pin_mut!(parsed);
            while let Some(event) = parsed.next().await {
                yield event;
            }
            completed.store(true, Ordering::Relaxed);
        };

        Ok(Box::pin(final_stream))
    }

    /// Stored token state for the `auth status` command.
    pub async fn token_status(&self) -> Result<Option<TokenStatus>> {
        let Some(row) = auth_tokens::get(&self.pool, &self.user_id).await? else {
            return Ok(None);
        };
        let expired = DateTime::parse_from_rfc3339(&row.expires_at)
            .map(|e| e.with_timezone(&Utc) <= Utc::now())
            .unwrap_or(true);
        Ok(Some(TokenStatus {
            account_id: row.account_id,
            expires_at: row.expires_at,
            expired,
            updated_at: row.updated_at,
        }))
    }

    /// Current access token, refreshed if it is at or near expiry.
    async fn access_token(&self) -> Result<String> {
        let mut cached = self.cached.lock().await;

        if cached.is_none() {
            let row = auth_tokens::get(&self.pool, &self.user_id)
                .await?
                .ok_or_else(|| {
                    anyhow!(
                        "codex is not authenticated for user '{}'. \
                         Run `leek auth login` (or `leek auth import`) first.",
                        self.user_id
                    )
                })?;
            let expires_at = DateTime::parse_from_rfc3339(&row.expires_at)
                .map(|e| e.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            *cached = Some(CodexTokens {
                access_token: row.access_token,
                refresh_token: row.refresh_token,
                account_id: row.account_id,
                expires_at,
            });
        }

        let skew = chrono::TimeDelta::seconds(REFRESH_SKEW_SECS);
        let needs_refresh = cached
            .as_ref()
            .map(|t| t.expires_at - Utc::now() < skew)
            .unwrap_or(false);

        if needs_refresh {
            let current = cached.as_ref().unwrap().clone();
            tracing::info!(expires_at = %current.expires_at, "refreshing codex access token");
            let fresh = oauth::refresh(&self.http, &current.refresh_token)
                .await
                .context("refreshing the codex access token")?;
            auth_tokens::upsert(
                &self.pool,
                &self.user_id,
                &fresh.access_token,
                &fresh.refresh_token,
                fresh
                    .account_id
                    .as_deref()
                    .or(current.account_id.as_deref()),
                &fresh.expires_at.to_rfc3339(),
            )
            .await?;
            *cached = Some(fresh);
        }

        Ok(cached.as_ref().unwrap().access_token.clone())
    }
}

/// RAII guard for the F2 transcript: when this drops (either because the
/// wrapper stream ran to completion or because the consumer cancelled
/// it), spawn a task to finalize the row with the bytes accumulated so
/// far. `Drop` runs synchronously so the async `finalize` is spawned —
/// the drop happens inside the agent loop's tokio task, which is always
/// in a tokio runtime, so the spawn is safe.
struct TranscriptGuard {
    pool: SqlitePool,
    row_id: i64,
    buffer: Arc<tokio::sync::Mutex<Vec<u8>>>,
    /// `true` once the wrapper stream's body ran to its end. `false` at
    /// drop time means the consumer dropped us early (or the upstream
    /// raised an SSE error and the stream terminated abnormally).
    completed: Arc<AtomicBool>,
}

impl Drop for TranscriptGuard {
    fn drop(&mut self) {
        let pool = self.pool.clone();
        let row_id = self.row_id;
        let buffer = self.buffer.clone();
        let status: i64 = if self.completed.load(Ordering::Relaxed) {
            200
        } else {
            // Sentinel: the stream did not run to completion — partial
            // body is still archived, but the status flags incompleteness.
            0
        };
        tokio::spawn(async move {
            let buf = buffer.lock().await.clone();
            let now = chrono::Utc::now().to_rfc3339();
            if let Err(e) =
                llm_transcripts::finalize(&pool, row_id, &buf, status, &now).await
            {
                tracing::warn!(
                    error = %e,
                    row_id,
                    "finalizing llm_transcript row failed (background)"
                );
            }
        });
    }
}

/// UTF-8-safe char truncation. The error body is often JSON or HTML —
/// slicing by bytes can blow up at a multibyte CJK boundary.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// Scrub `Bearer <token>` runs out of a string before it lands in a log
/// line or an SSE error payload. Defensive — the codex backend does not
/// echo the Authorization header today, but a leaked live token is costly.
fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("Bearer ") {
        out.push_str(&rest[..i + "Bearer ".len()]);
        let after = &rest[i + "Bearer ".len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .unwrap_or(after.len());
        out.push_str("<redacted>");
        rest = &after[end..];
    }
    out.push_str(rest);
    if out.len() > 600 {
        let mut end = 600;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{redact, truncate_chars, CodexCallError};

    #[test]
    fn redact_scrubs_bearer_tokens() {
        let out = redact("error: invalid Bearer abc.def-ghi_123 token");
        assert!(!out.contains("abc.def-ghi_123"));
        assert!(out.contains("Bearer <redacted>"));
    }

    #[test]
    fn redact_passes_clean_strings() {
        assert_eq!(redact("plain error, no secrets"), "plain error, no secrets");
    }

    #[test]
    fn truncate_chars_caps_long_strings() {
        let out = truncate_chars(&"a".repeat(500), 200);
        // 200 chars + ellipsis
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_chars_passes_short_strings() {
        assert_eq!(truncate_chars("hi", 200), "hi");
    }

    #[test]
    fn truncate_chars_respects_cjk_boundary() {
        let s = "中".repeat(300);
        let out = truncate_chars(&s, 100);
        // 100 CJK chars + ellipsis — slicing by bytes would have panicked.
        assert_eq!(out.chars().count(), 101);
    }

    #[test]
    fn http_error_displays_status_and_body() {
        let e = CodexCallError::Http {
            status: 503,
            body_excerpt: "Service Unavailable".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("503"));
        assert!(s.contains("Service Unavailable"));
    }

    #[test]
    fn connection_error_displays_detail() {
        let e = CodexCallError::Connection {
            detail: "dns lookup error: no such host".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("dns"));
    }

    #[test]
    fn call_error_round_trips_through_anyhow() {
        let inner = CodexCallError::Http {
            status: 401,
            body_excerpt: "expired".into(),
        };
        let anyhow_err = anyhow::Error::new(inner);
        let downcast = anyhow_err.downcast_ref::<CodexCallError>();
        match downcast {
            Some(CodexCallError::Http { status, body_excerpt }) => {
                assert_eq!(*status, 401);
                assert_eq!(body_excerpt, "expired");
            }
            other => panic!("expected Http variant, got {other:?}"),
        }
    }
}
