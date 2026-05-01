//! Event row helpers — durable record of every event emitted by the gateway.
//!
//! `events.seq` is per-session monotonic and **independent** from `messages.seq`.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

pub async fn insert(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    kind: &str,
    payload: &serde_json::Value,
    source: Option<&str>,
    ts: chrono::DateTime<chrono::Utc>,
) -> Result<i64> {
    let mut tx = pool.begin().await?;

    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE user_id = ? AND session_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await
    .context("computing next event seq")?;

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO events
          (user_id, session_id, seq, task_id, kind, payload_json, source, ts, persisted_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(seq)
    .bind(task_id)
    .bind(kind)
    .bind(payload.to_string())
    .bind(source)
    .bind(ts.to_rfc3339())
    .bind(&now)
    .execute(&mut *tx)
    .await
    .context("inserting event")?;

    tx.commit().await?;
    Ok(seq)
}
