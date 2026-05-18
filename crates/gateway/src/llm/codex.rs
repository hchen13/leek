//! `CodexClient` — the concrete client for the codex backend.
//!
//! One client, no trait (ARCHITECTURE §2). It owns token lifecycle (load
//! from the vault, refresh near expiry) and turns a `ChatRequest` into a
//! streamed `LlmEvent` flow over the OpenAI Responses API.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use super::oauth::{self, CodexTokens};
use super::{responses, ChatRequest, LlmEvent};
use crate::vault::auth_tokens;

/// codex backend Responses endpoint.
const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
/// Refresh the access token once it is within this many seconds of expiry.
const REFRESH_SKEW_SECS: i64 = 120;

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
        // No total request timeout: a streamed turn legitimately runs for
        // minutes, and the agent loop's idle-timeout guard owns responsiveness.
        // A connect timeout still catches a dead network fast.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(15))
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
    pub async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let access_token = self.access_token().await?;
        let body = responses::build_request_body(&req);

        let resp = self
            .http
            .post(RESPONSES_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .context("POST to the codex Responses endpoint")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("codex backend returned {status}: {}", redact(&text));
        }
        Ok(responses::parse_sse_stream(resp))
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
    use super::redact;

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
}
