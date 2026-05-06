//! `codex_oauth` provider — talks to ChatGPT subscription backend.
//!
//! See `design/p1-spec/llm-provider.md` §4.3 + §5 for protocol details.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::auth::codex::{self, CodexTokens};
use crate::vault::provider_configs;

use super::openai_responses;
use super::{ChatRequest, LlmEvent, LlmProvider};

const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const REFRESH_SKEW_SECS: i64 = 60;

pub struct CodexOauthProvider {
    pool: SqlitePool,
    user_id: String,
    http: reqwest::Client,
    cached: Arc<Mutex<Option<CodexTokens>>>,
}

impl CodexOauthProvider {
    pub fn new(pool: SqlitePool, user_id: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            // Long-running SSE streams shouldn't time out mid-response.
            // Per-request timeout is conservative; kept-alive idle is short.
            .pool_idle_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()
            .context("building reqwest client for codex_oauth")?;
        Ok(Self {
            pool,
            user_id: user_id.into(),
            http,
            cached: Arc::new(Mutex::new(None)),
        })
    }

    /// Return current access_token, refreshing if it's near expiry.
    async fn ensure_fresh_token(&self) -> Result<String> {
        let mut cached = self.cached.lock().await;

        if cached.is_none() {
            let row = provider_configs::get_codex(&self.pool, &self.user_id)
                .await?
                .ok_or_else(|| {
                    anyhow!(
                        "codex_oauth not configured for user '{}'. Run `leek auth codex` first.",
                        self.user_id
                    )
                })?;
            *cached = Some(CodexTokens {
                access_token: row.access_token,
                refresh_token: row.refresh_token,
                expires_at: row.expires_at,
            });
        }

        if needs_refresh(cached.as_ref().unwrap().expires_at) {
            let current = cached.as_ref().unwrap().clone();
            tracing::info!(
                expires_at = %current.expires_at,
                "refreshing codex access_token"
            );
            let new = codex::refresh(&self.http, &current.refresh_token)
                .await
                .context("refreshing codex token")?;
            provider_configs::upsert_codex(&self.pool, &self.user_id, &new).await?;
            *cached = Some(new);
        }

        Ok(cached.as_ref().unwrap().access_token.clone())
    }
}

fn needs_refresh(expires_at: DateTime<Utc>) -> bool {
    let skew = chrono::Duration::seconds(REFRESH_SKEW_SECS);
    expires_at - Utc::now() < skew
}

#[async_trait]
impl LlmProvider for CodexOauthProvider {
    fn name(&self) -> &str {
        "codex_oauth"
    }

    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let access_token = self.ensure_fresh_token().await?;
        let body = openai_responses::build_request_body(&req);
        let url = format!("{BASE_URL}/responses");

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .context("POST /responses to codex backend")?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            bail!("codex /responses returned {status}: {body_text}");
        }

        Ok(openai_responses::parse_sse_stream(resp))
    }
}
