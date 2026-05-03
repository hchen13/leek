//! Event row helpers — durable record of every event emitted by the gateway.
//!
//! `events.seq` is per-session monotonic and **independent** from `messages.seq`.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

/// Wire shape returned by the events list endpoint.
#[derive(Debug, FromRow, Serialize)]
pub struct EventRow {
    pub seq: i64,
    #[sqlx(default)]
    pub task_id: Option<String>,
    pub kind: String,
    pub payload_json: String,
    #[sqlx(default)]
    pub source: Option<String>,
    pub ts: String,
}

pub async fn list_for_session(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    since: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<EventRow>> {
    // Cap is generous: chat history reload wants the entire session's
    // event log so tool calls / narrations rendered before reload don't
    // disappear. SQLite handles 50k row scans easily over a session_id+seq
    // index.
    let limit = limit.unwrap_or(20_000).clamp(1, 100_000);
    let rows: Vec<EventRow> = sqlx::query_as(
        r#"
        SELECT seq, task_id, kind, payload_json, source, ts
        FROM events
        WHERE user_id = ?
          AND session_id = ?
          AND seq > ?
        ORDER BY seq ASC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(since.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing events")?;
    Ok(rows)
}

/// Most recent `input_tokens` reported by the LLM provider for this session.
/// Used by the auto-compaction trigger: when the session's last LLM call
/// already saw > N tokens of input, the next turn would exceed the budget,
/// so compact before sending it. Returns `None` for fresh sessions with no
/// `llm_usage` events yet.
pub async fn latest_input_tokens(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<Option<i64>> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT payload_json
        FROM events
        WHERE user_id = ?
          AND session_id = ?
          AND kind = 'llm_usage'
        ORDER BY seq DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .context("reading latest llm_usage event")?;

    let Some((payload_json,)) = row else {
        return Ok(None);
    };
    let payload: serde_json::Value =
        serde_json::from_str(&payload_json).context("parsing llm_usage payload")?;
    Ok(payload.get("input_tokens").and_then(|v| v.as_i64()))
}

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
