//! Message rows. `seq` is per-session monotonic.

use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Serialize, sqlx::FromRow)]
pub struct Message {
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Append a message to a session. The per-session `seq` is allocated
/// atomically (`MAX(seq) + 1`) inside the insert.
pub async fn insert(
    pool: &SqlitePool,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<Message> {
    let now = chrono::Utc::now().to_rfc3339();
    let msg = sqlx::query_as::<_, Message>(
        "INSERT INTO messages (session_id, seq, role, content, created_at) \
         VALUES (?, (SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?), ?, ?, ?) \
         RETURNING seq, role, content, created_at",
    )
    .bind(session_id)
    .bind(session_id)
    .bind(role)
    .bind(content)
    .bind(&now)
    .fetch_one(pool)
    .await?;
    Ok(msg)
}

/// Messages with `seq` greater than `since` (default 0), ascending, capped.
pub async fn list(
    pool: &SqlitePool,
    session_id: &str,
    since: Option<i64>,
    limit: i64,
) -> Result<Vec<Message>> {
    let rows = sqlx::query_as::<_, Message>(
        "SELECT seq, role, content, created_at FROM messages \
         WHERE session_id = ? AND seq > ? ORDER BY seq ASC LIMIT ?",
    )
    .bind(session_id)
    .bind(since.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
