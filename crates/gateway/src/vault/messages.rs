//! Message row helpers.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MessageRow {
    pub seq: i64,
    pub task_id: Option<String>,
    pub role: String,
    pub content_json: String,
    pub created_at: String,
}

pub async fn insert(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    role: &str,
    content_json: &serde_json::Value,
    task_id: Option<&str>,
) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;

    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE user_id = ? AND session_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await
    .context("computing next message seq")?;

    sqlx::query(
        "INSERT INTO messages (user_id, session_id, seq, task_id, role, content_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(session_id)
    .bind(seq)
    .bind(task_id)
    .bind(role)
    .bind(content_json.to_string())
    .bind(&now)
    .execute(&mut *tx)
    .await
    .context("inserting message")?;

    tx.commit().await?;
    Ok(seq)
}

pub async fn list(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    since_seq: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageRow>> {
    let rows: Vec<MessageRow> = sqlx::query_as(
        r#"
        SELECT seq, task_id, role, content_json, created_at
        FROM messages
        WHERE user_id = ? AND session_id = ? AND seq > ?
        ORDER BY seq ASC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(since_seq.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing messages")?;
    Ok(rows)
}
