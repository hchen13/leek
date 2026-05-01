//! Read/write `llm_provider_configs` rows for `codex_oauth`.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::auth::codex::CodexTokens;

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields read by codex_oauth provider in Task #44
pub struct CodexConfig {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn upsert_codex(
    pool: &SqlitePool,
    user_id: &str,
    tokens: &CodexTokens,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO llm_provider_configs
          (user_id, provider_name, auth_kind,
           oauth_access_token, oauth_refresh_token, oauth_expires_at,
           priority, enabled, created_at, updated_at)
        VALUES (?, 'codex_oauth', 'oauth', ?, ?, ?, 100, 1, ?, ?)
        ON CONFLICT(user_id, provider_name) DO UPDATE SET
          auth_kind           = 'oauth',
          oauth_access_token  = excluded.oauth_access_token,
          oauth_refresh_token = excluded.oauth_refresh_token,
          oauth_expires_at    = excluded.oauth_expires_at,
          enabled             = 1,
          last_error          = NULL,
          last_error_at       = NULL,
          updated_at          = excluded.updated_at
        "#,
    )
    .bind(user_id)
    .bind(&tokens.access_token)
    .bind(&tokens.refresh_token)
    .bind(tokens.expires_at.to_rfc3339())
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .context("writing codex tokens to vault")?;
    Ok(())
}

#[allow(dead_code)] // wired up in Task #44 (codex_oauth provider) — needed before each LLM call
pub async fn get_codex(pool: &SqlitePool, user_id: &str) -> Result<Option<CodexConfig>> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        r#"
        SELECT oauth_access_token, oauth_refresh_token, oauth_expires_at
        FROM llm_provider_configs
        WHERE user_id = ? AND provider_name = 'codex_oauth' AND enabled = 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("reading codex tokens from vault")?;

    match row {
        None => Ok(None),
        Some((access, refresh, exp)) => {
            let expires_at = DateTime::parse_from_rfc3339(&exp)
                .map_err(|e| anyhow!("invalid expires_at in vault: {e}"))?
                .with_timezone(&Utc);
            Ok(Some(CodexConfig {
                access_token: access,
                refresh_token: refresh,
                expires_at,
            }))
        }
    }
}
