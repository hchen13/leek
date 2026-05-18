//! Event rows — the durable log behind the SSE stream. `seq` is per-session
//! monotonic and independent from `messages.seq`.

use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqlitePool};

#[derive(Clone, Serialize)]
pub struct Event {
    pub seq: i64,
    pub kind: String,
    /// JSON object — stored as TEXT in SQLite, surfaced as nested JSON here.
    pub payload: serde_json::Value,
    pub created_at: String,
}

fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Event> {
    let payload: String = row.try_get("payload")?;
    Ok(Event {
        seq: row.try_get("seq")?,
        kind: row.try_get("kind")?,
        payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
        created_at: row.try_get("created_at")?,
    })
}

/// Append an event to a session and return the stored row.
pub async fn insert(
    pool: &SqlitePool,
    session_id: &str,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<Event> {
    let now = chrono::Utc::now().to_rfc3339();
    let payload_text = serde_json::to_string(payload)?;
    let row = sqlx::query(
        "INSERT INTO events (session_id, seq, kind, payload, created_at) \
         VALUES (?, (SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE session_id = ?), ?, ?, ?) \
         RETURNING seq, kind, payload, created_at",
    )
    .bind(session_id)
    .bind(session_id)
    .bind(kind)
    .bind(&payload_text)
    .bind(&now)
    .fetch_one(pool)
    .await?;
    from_row(&row)
}

/// Events with `seq` greater than `since` (default 0), ascending, capped.
pub async fn list(
    pool: &SqlitePool,
    session_id: &str,
    since: Option<i64>,
    limit: i64,
) -> Result<Vec<Event>> {
    let rows = sqlx::query(
        "SELECT seq, kind, payload, created_at FROM events \
         WHERE session_id = ? AND seq > ? ORDER BY seq ASC LIMIT ?",
    )
    .bind(session_id)
    .bind(since.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter().map(from_row).collect()
}
