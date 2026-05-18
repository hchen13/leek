//! Codex OAuth — device-authorization flow, token refresh, and an optional
//! one-shot import from the codex CLI's own `~/.codex/auth.json`.
//!
//! There is no provider abstraction here (ARCHITECTURE §2): this is the
//! concrete codex `chatgpt`-mode OAuth, and nothing else.
//!
//! ## Token-refresh caveat (important)
//!
//! A codex OAuth *refresh token* is single-holder. When it is used to mint a
//! new access token the provider may rotate it, which invalidates every
//! other copy. Two consequences:
//!
//!  - `auth login` runs leek's *own* device flow and stores an independent
//!    token set — this is the clean path; it never disturbs the codex CLI.
//!  - `auth import` copies the codex CLI's current tokens into the leek
//!    vault. They then share one refresh token: whichever process refreshes
//!    first wins, and the other (codex CLI *or* leek) will have to log in
//!    again. Import is a convenience for quick verification, not a
//!    long-term arrangement — prefer `auth login`.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const VERIFICATION_URL: &str = "https://auth.openai.com/codex/device";
const REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

const POLL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POLL_INTERVAL_MIN_SECS: u64 = 3;
const POLL_INTERVAL_DEFAULT_SECS: u64 = 5;

/// A codex token set, ready to store in the vault.
#[derive(Debug, Clone)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: Option<String>,
    pub expires_at: DateTime<Utc>,
}

/// Claims decoded (unverified) from a codex access-token JWT.
pub struct JwtClaims {
    pub expires_at: DateTime<Utc>,
    pub account_id: Option<String>,
}

/// Decode the `exp` and codex account id from a JWT *without verifying the
/// signature* — verification would need OpenAI's JWKS, and the API rejects
/// a bad token anyway. We only read `exp` to decide when to refresh.
pub fn decode_jwt(token: &str) -> Result<JwtClaims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        bail!("not a JWT (expected 3 dot-separated segments)");
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .context("JWT payload is not valid base64url")?;
    let claims: serde_json::Value =
        serde_json::from_slice(&payload).context("JWT payload is not valid JSON")?;

    let exp = claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("JWT has no integer 'exp' claim"))?;
    let expires_at = Utc
        .timestamp_opt(exp, 0)
        .single()
        .ok_or_else(|| anyhow!("JWT 'exp' out of range"))?;

    // codex account id lives under the `https://api.openai.com/auth` claim.
    let account_id = claims
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(JwtClaims {
        expires_at,
        account_id,
    })
}

// ── device-authorization flow ───────────────────────────────────────────

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

/// Run the interactive device-authorization flow: print a URL + code, poll
/// until the user authorizes, then exchange the code for tokens. Blocks up
/// to 15 minutes.
pub async fn device_flow_login() -> Result<CodexTokens> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("building HTTP client for device flow")?;

    let usercode = request_user_code(&client).await?;
    let interval = parse_interval(&usercode.interval).max(POLL_INTERVAL_MIN_SECS);

    println!();
    println!("  To authorize leek:");
    println!("    1. open  {VERIFICATION_URL}");
    println!("    2. enter code  {}", usercode.user_code);
    println!();
    println!("  Waiting for authorization (15 min timeout, Ctrl+C to cancel)…");

    let poll = poll_device_token(&client, &usercode, interval).await?;
    let token = exchange_code(&client, &poll).await?;
    let refresh = token
        .refresh_token
        .ok_or_else(|| anyhow!("token exchange returned no refresh_token"))?;
    let claims = decode_jwt(&token.access_token).context("decoding the new access token")?;

    Ok(CodexTokens {
        access_token: token.access_token,
        refresh_token: refresh,
        account_id: claims.account_id,
        expires_at: claims.expires_at,
    })
}

/// Refresh an access token. The provider may rotate the refresh token; the
/// returned `CodexTokens` carries whichever refresh token is now valid (the
/// new one if the response gave one, else the one passed in).
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
        .context("codex token refresh request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!(
            "codex token refresh returned {status}: {}",
            truncate(&body, 300)
        );
    }

    let token: TokenResponse = resp.json().await.context("invalid refresh response")?;
    let new_refresh = token
        .refresh_token
        .unwrap_or_else(|| refresh_token.to_string());
    let claims = decode_jwt(&token.access_token).context("decoding the refreshed access token")?;

    Ok(CodexTokens {
        access_token: token.access_token,
        refresh_token: new_refresh,
        account_id: claims.account_id,
        expires_at: claims.expires_at,
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
        bail!("device usercode request returned {}", resp.status());
    }
    resp.json().await.context("invalid usercode response")
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
            bail!("device authorization timed out after 15 minutes");
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
            .context("device poll request failed")?;

        match resp.status().as_u16() {
            200 => return resp.json().await.context("invalid device poll response"),
            403 | 404 => continue, // not authorized yet
            other => bail!("device poll returned unexpected status {other}"),
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
        .context("authorization-code exchange failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("code exchange returned {status}: {}", truncate(&body, 300));
    }
    resp.json().await.context("invalid token-exchange response")
}

// ── import from the codex CLI ───────────────────────────────────────────

#[derive(Deserialize)]
struct CodexCliAuth {
    tokens: Option<CodexCliTokens>,
}

#[derive(Deserialize)]
struct CodexCliTokens {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    account_id: Option<String>,
}

/// Path to the codex CLI's auth file (`~/.codex/auth.json`).
pub fn codex_cli_auth_path() -> Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("$HOME is not set"))?;
    Ok(std::path::PathBuf::from(home)
        .join(".codex")
        .join("auth.json"))
}

/// Import the codex CLI's current tokens. See the module-level refresh caveat
/// — the imported tokens share a refresh token with the codex CLI.
pub fn import_from_codex_cli() -> Result<CodexTokens> {
    let path = codex_cli_auth_path()?;
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let auth: CodexCliAuth =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    let tokens = auth.tokens.ok_or_else(|| {
        anyhow!(
            "{} has no `tokens` object — is the codex CLI logged in?",
            path.display()
        )
    })?;

    let claims = decode_jwt(&tokens.access_token).context("decoding the imported access token")?;
    Ok(CodexTokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        // Prefer the file's account_id; fall back to the JWT claim.
        account_id: tokens.account_id.or(claims.account_id),
        expires_at: claims.expires_at,
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_jwt_reads_exp_and_account() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"exp":1893456000,"https://api.openai.com/auth":{"chatgpt_account_id":"acct-9"}}"#,
        );
        let token = format!("{header}.{payload}.");
        let claims = decode_jwt(&token).unwrap();
        assert_eq!(claims.expires_at.timestamp(), 1893456000);
        assert_eq!(claims.account_id.as_deref(), Some("acct-9"));
    }

    #[test]
    fn decode_jwt_tolerates_missing_account() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(br#"{"exp":1893456000}"#);
        let token = format!("{header}.{payload}.");
        let claims = decode_jwt(&token).unwrap();
        assert!(claims.account_id.is_none());
    }

    #[test]
    fn decode_jwt_rejects_non_jwt() {
        assert!(decode_jwt("not-a-jwt").is_err());
    }
}
