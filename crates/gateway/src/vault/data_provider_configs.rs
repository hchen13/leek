use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, FromRow, Serialize)]
pub struct DataProviderStatus {
    pub provider_name: String,
    pub source: String,
    pub configured: bool,
    pub enabled: bool,
    pub token_last4: Option<String>,
    pub updated_at: String,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
}

pub async fn get_api_key(
    pool: &SqlitePool,
    user_id: &str,
    provider_name: &str,
) -> Result<Option<String>> {
    let row: Option<(Option<String>, i64)> = sqlx::query_as(
        r#"
        SELECT api_key, enabled
        FROM data_provider_configs
        WHERE user_id = ? AND provider_name = ?
        "#,
    )
    .bind(user_id)
    .bind(provider_name)
    .fetch_optional(pool)
    .await
    .context("reading data provider api key")?;

    let Some((api_key, enabled)) = row else {
        return Ok(None);
    };
    if enabled != 1 {
        return Ok(None);
    }
    Ok(api_key.filter(|key| !key.trim().is_empty()))
}

pub async fn get_status(
    pool: &SqlitePool,
    user_id: &str,
    provider_name: &str,
) -> Result<Option<DataProviderStatus>> {
    let row: Option<(
        String,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
        String,
    )> = sqlx::query_as(
        r#"
            SELECT provider_name, api_key, enabled, last_error, last_error_at, updated_at
            FROM data_provider_configs
            WHERE user_id = ? AND provider_name = ?
            "#,
    )
    .bind(user_id)
    .bind(provider_name)
    .fetch_optional(pool)
    .await
    .context("reading data provider status")?;

    Ok(row.map(
        |(provider_name, api_key, enabled, last_error, last_error_at, updated_at)| {
            let token_last4 = api_key
                .as_deref()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(last4);
            DataProviderStatus {
                provider_name,
                source: "vault".to_string(),
                configured: token_last4.is_some(),
                enabled: enabled == 1,
                token_last4,
                updated_at,
                last_error,
                last_error_at,
            }
        },
    ))
}

pub async fn upsert_api_key(
    pool: &SqlitePool,
    user_id: &str,
    provider_name: &str,
    api_key: &str,
    enabled: bool,
) -> Result<DataProviderStatus> {
    let token = api_key.trim();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO data_provider_configs
          (user_id, provider_name, api_key, enabled, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(user_id, provider_name) DO UPDATE SET
          api_key = excluded.api_key,
          enabled = excluded.enabled,
          last_error = NULL,
          last_error_at = NULL,
          updated_at = excluded.updated_at
        "#,
    )
    .bind(user_id)
    .bind(provider_name)
    .bind(token)
    .bind(if enabled { 1 } else { 0 })
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .context("writing data provider api key")?;

    get_status(pool, user_id, provider_name)
        .await?
        .context("data provider missing after upsert")
}

pub async fn set_enabled(
    pool: &SqlitePool,
    user_id: &str,
    provider_name: &str,
    enabled: bool,
) -> Result<Option<DataProviderStatus>> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE data_provider_configs
        SET enabled = ?, updated_at = ?
        WHERE user_id = ? AND provider_name = ?
        "#,
    )
    .bind(if enabled { 1 } else { 0 })
    .bind(&now)
    .bind(user_id)
    .bind(provider_name)
    .execute(pool)
    .await
    .context("updating data provider enabled flag")?;

    get_status(pool, user_id, provider_name).await
}

fn last4(token: &str) -> String {
    let mut chars: Vec<char> = token.chars().rev().take(4).collect();
    chars.reverse();
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE data_provider_configs (
                user_id TEXT NOT NULL,
                provider_name TEXT NOT NULL,
                api_key TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_error TEXT,
                last_error_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, provider_name)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn api_key_round_trips_without_status_leaking_full_token() {
        let pool = pool().await;
        let status = upsert_api_key(&pool, "u", "tushare", "abcdef123456", true)
            .await
            .unwrap();
        assert!(status.configured);
        assert_eq!(status.token_last4.as_deref(), Some("3456"));

        let key = get_api_key(&pool, "u", "tushare").await.unwrap();
        assert_eq!(key.as_deref(), Some("abcdef123456"));
    }

    #[tokio::test]
    async fn disabled_provider_returns_no_api_key() {
        let pool = pool().await;
        upsert_api_key(&pool, "u", "tushare", "abcdef123456", true)
            .await
            .unwrap();
        set_enabled(&pool, "u", "tushare", false).await.unwrap();

        let key = get_api_key(&pool, "u", "tushare").await.unwrap();
        assert!(key.is_none());
    }
}
