//! Session row helpers.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

pub async fn ensure_exists(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    title: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO sessions (user_id, id, title, status, pinned, created_at, last_active_at)
        VALUES (?, ?, ?, 'active', 0, ?, ?)
        ON CONFLICT(user_id, id) DO UPDATE SET last_active_at = excluded.last_active_at
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(title)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .context("ensuring session row")?;
    Ok(())
}
