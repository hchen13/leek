//! Codex OAuth token storage — one row per user in `codex_auth`.
//!
//! The gateway authenticates to the codex backend with a `chatgpt` OAuth
//! access token. The token (and the refresh token that renews it) must
//! survive a restart, so they live in the vault rather than in memory.
//! `expires_at` is the access_token's JWT `exp`; `CodexClient` refreshes
//! when the current time is within a small skew of it.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// A stored codex token set.
#[derive(Debug, Clone)]
pub struct CodexAuthRow {
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: Option<String>,
    /// RFC3339 — the access_token JWT `exp`.
    pub expires_at: String,
    pub updated_at: String,
}

/// Read the codex token row for a user, if one has been stored.
pub async fn get(pool: &SqlitePool, user_id: &str) -> Result<Option<CodexAuthRow>> {
    let row = sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
        "SELECT access_token, refresh_token, account_id, expires_at, updated_at \
         FROM codex_auth WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("reading codex_auth row")?;

    Ok(row.map(
        |(access_token, refresh_token, account_id, expires_at, updated_at)| CodexAuthRow {
            access_token,
            refresh_token,
            account_id,
            expires_at,
            updated_at,
        },
    ))
}

/// Insert or replace the codex token row for a user.
pub async fn upsert(
    pool: &SqlitePool,
    user_id: &str,
    access_token: &str,
    refresh_token: &str,
    account_id: Option<&str>,
    expires_at: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO codex_auth \
           (user_id, access_token, refresh_token, account_id, expires_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE SET \
           access_token = excluded.access_token, \
           refresh_token = excluded.refresh_token, \
           account_id = excluded.account_id, \
           expires_at = excluded.expires_at, \
           updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(access_token)
    .bind(refresh_token)
    .bind(account_id)
    .bind(expires_at)
    .bind(&now)
    .execute(pool)
    .await
    .context("upserting codex_auth row")?;
    Ok(())
}
