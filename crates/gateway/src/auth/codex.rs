//! Codex OAuth device flow — protocol verified against hermes-agent.
//!
//! See `design/p1-spec/llm-provider.md` §5 for the full protocol description.
//! Critical: codex CLI / VS Code extension / leek **cannot** share a refresh
//! token (rotation invalidates the others). We run our own device flow and
//! store tokens in vault, isolated from `~/.codex/auth.json`.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::time::{Duration, Instant};

use super::jwt;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

const POLL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POLL_INTERVAL_DEFAULT_SECS: u64 = 5;
const POLL_INTERVAL_MIN_SECS: u64 = 3;

#[derive(Debug, Clone)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct UsercodeResponse {
    user_code: String,
    device_auth_id: String,
    #[serde(default)]
    interval: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct DevicePollResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

pub async fn device_flow_login() -> Result<CodexTokens> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let usercode = request_user_code(&client).await?;
    let interval_secs = parse_interval(&usercode.interval).max(POLL_INTERVAL_MIN_SECS);

    println!();
    println!("To continue:");
    println!();
    println!("  1. Open in your browser:");
    println!("     \x1b[94m{VERIFICATION_URL}\x1b[0m");
    println!();
    println!("  2. Enter this code:");
    println!("     \x1b[94m{}\x1b[0m", usercode.user_code);
    println!();
    println!("  Waiting for authorization (15 min timeout, Ctrl+C to cancel)...");

    let poll = poll_device_token(&client, &usercode, interval_secs).await?;
    let token = exchange_code(&client, &poll).await?;

    let refresh = token
        .refresh_token
        .ok_or_else(|| anyhow!("token exchange did not return refresh_token"))?;
    let expires_at =
        jwt::decode_exp(&token.access_token).context("decoding exp from access_token JWT")?;

    Ok(CodexTokens {
        access_token: token.access_token,
        refresh_token: refresh,
        expires_at,
    })
}

/// Refresh access_token. `refresh_token` may rotate — caller should persist
/// whichever the response returned (or the existing one if response omitted it).
#[allow(dead_code)] // wired up in Task #44 (codex_oauth provider)
pub async fn refresh(client: &reqwest::Client, refresh_token: &str) -> Result<CodexTokens> {
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .context("refresh request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let code = extract_error_code(&body);
        if matches!(
            code.as_deref(),
            Some("invalid_grant" | "invalid_token" | "invalid_request" | "refresh_token_reused")
        ) {
            bail!(
                "refresh failed (relogin required): {} — body: {}",
                code.unwrap_or_default(),
                body
            );
        }
        bail!("refresh returned status {} body: {}", status, body);
    }

    let token: TokenResponse = resp.json().await.context("invalid refresh response")?;
    let new_refresh = token
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string());
    let expires_at =
        jwt::decode_exp(&token.access_token).context("decoding exp from refreshed access_token")?;

    Ok(CodexTokens {
        access_token: token.access_token,
        refresh_token: new_refresh,
        expires_at,
    })
}

async fn request_user_code(client: &reqwest::Client) -> Result<UsercodeResponse> {
    let resp = client
        .post(DEVICE_USERCODE_URL)
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await
        .context("device usercode request failed")?;
    if !resp.status().is_success() {
        bail!("device usercode returned status {}", resp.status());
    }
    resp.json::<UsercodeResponse>()
        .await
        .context("invalid usercode response")
}

fn parse_interval(v: &Option<serde_json::Value>) -> u64 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(POLL_INTERVAL_DEFAULT_SECS),
        Some(serde_json::Value::String(s)) => s.parse().unwrap_or(POLL_INTERVAL_DEFAULT_SECS),
        _ => POLL_INTERVAL_DEFAULT_SECS,
    }
}

async fn poll_device_token(
    client: &reqwest::Client,
    usercode: &UsercodeResponse,
    interval_secs: u64,
) -> Result<DevicePollResponse> {
    let start = Instant::now();
    loop {
        if start.elapsed() >= POLL_TIMEOUT {
            bail!("login timed out after 15 minutes");
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;

        let resp = client
            .post(DEVICE_TOKEN_URL)
            .json(&serde_json::json!({
                "device_auth_id": usercode.device_auth_id,
                "user_code": usercode.user_code,
            }))
            .send()
            .await
            .context("poll request failed")?;

        match resp.status().as_u16() {
            200 => {
                return resp
                    .json::<DevicePollResponse>()
                    .await
                    .context("invalid poll response")
            }
            403 | 404 => continue,
            other => bail!("device poll returned unexpected status {}", other),
        }
    }
}

async fn exchange_code(
    client: &reqwest::Client,
    poll: &DevicePollResponse,
) -> Result<TokenResponse> {
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", poll.authorization_code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", poll.code_verifier.as_str()),
        ])
        .send()
        .await
        .context("token exchange failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("token exchange returned status {} body: {}", status, body);
    }
    resp.json::<TokenResponse>()
        .await
        .context("invalid token response")
}

#[allow(dead_code)] // helper for refresh()
fn extract_error_code(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let err = v.get("error")?;
    if let Some(s) = err.as_str() {
        return Some(s.to_string());
    }
    err.get("code")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}
