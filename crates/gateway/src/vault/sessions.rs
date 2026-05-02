//! Session row helpers.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, FromRow, Serialize, Clone)]
pub struct SessionRow {
    pub id: String,
    pub title: Option<String>,
    pub status: String,
    pub pinned: i64,
    pub created_at: String,
    pub last_active_at: String,
}

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

pub async fn list(pool: &SqlitePool, user_id: &str) -> Result<Vec<SessionRow>> {
    let rows: Vec<SessionRow> = sqlx::query_as(
        r#"
        SELECT id, title, status, pinned, created_at, last_active_at
        FROM sessions
        WHERE user_id = ? AND status != 'deleted'
        ORDER BY pinned DESC, last_active_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("listing sessions")?;
    Ok(rows)
}

pub async fn rename(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    new_title: &str,
) -> Result<()> {
    sqlx::query("UPDATE sessions SET title = ? WHERE user_id = ? AND id = ?")
        .bind(new_title)
        .bind(user_id)
        .bind(session_id)
        .execute(pool)
        .await
        .context("renaming session")?;
    Ok(())
}

/// Hard-delete a session and every row it owns (messages, events,
/// task ↔ session links). The vault has no foreign-key cascades, so we
/// fan out the deletion explicitly inside one transaction.
pub async fn hard_delete(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    for sql in [
        "DELETE FROM events WHERE user_id = ? AND session_id = ?",
        "DELETE FROM messages WHERE user_id = ? AND session_id = ?",
        "DELETE FROM tasks WHERE user_id = ? AND session_id = ?",
        "DELETE FROM sessions WHERE user_id = ? AND id = ?",
    ] {
        sqlx::query(sql)
            .bind(user_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("deleting session rows: {sql}"))?;
    }
    tx.commit().await?;
    Ok(())
}
