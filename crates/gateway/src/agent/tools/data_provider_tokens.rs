use anyhow::{Result, anyhow};

use crate::vault::data_provider_configs;

use super::ToolContext;

pub async fn tushare_token(ctx: &ToolContext) -> Result<String> {
    if let Some(status) =
        data_provider_configs::get_status(&ctx.pool, &ctx.user_id, "tushare").await?
    {
        if !status.enabled {
            return Err(anyhow!(
                "A-share data source is disabled in Settings. Enable it before using A-share data tools."
            ));
        }
        if let Some(token) =
            data_provider_configs::get_api_key(&ctx.pool, &ctx.user_id, "tushare").await?
        {
            return Ok(token);
        }
        return Err(anyhow!(
            "A-share data source is not configured. Add credentials in Settings > Data sources."
        ));
    }

    std::env::var("TUSHARE_TOKEN").map_err(|_| {
        anyhow!(
            "A-share data source is not configured. Add credentials in Settings > Data sources."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ctx() -> ToolContext {
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
        ToolContext {
            pool,
            event_bus: crate::events::EventBus::new(),
            user_id: "u".to_string(),
            session_id: "s".to_string(),
            task_id: None,
        }
    }

    #[tokio::test]
    async fn vault_token_wins() {
        let ctx = ctx().await;
        data_provider_configs::upsert_api_key(&ctx.pool, "u", "tushare", "vault-token", true)
            .await
            .unwrap();
        assert_eq!(tushare_token(&ctx).await.unwrap(), "vault-token");
    }

    #[tokio::test]
    async fn disabled_provider_blocks_fallback() {
        let ctx = ctx().await;
        data_provider_configs::upsert_api_key(&ctx.pool, "u", "tushare", "vault-token", false)
            .await
            .unwrap();
        let err = tushare_token(&ctx).await.unwrap_err().to_string();
        assert!(err.contains("disabled"));
    }
}
