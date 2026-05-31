//! Event row helpers — durable record of every event emitted by the gateway.
//!
//! `events.seq` is per-session monotonic and **independent** from `messages.seq`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use tokio::time::{sleep, Duration};

const INSERT_MAX_ATTEMPTS: usize = 6;
const INSERT_RETRY_BASE_MS: u64 = 10;

static EVENT_SEQ_LOCKS: OnceLock<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

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

/// Most recent `input_tokens` reported by the LLM provider for this session,
/// counting only calls AFTER the most recent compaction. Used by the pre-turn
/// auto-compaction trigger: when the session's last real LLM call already saw
/// > N tokens of input, the next turn would exceed the budget, so compact
/// before sending it.
///
/// The post-compaction filter is essential: a compaction resets the context,
/// so the pre-compaction input size no longer reflects what the next turn will
/// send. Without it the trigger loops forever — compact, still read the stale
/// over-threshold figure, compact again — and no user message ever gets in.
/// Returns `None` for fresh sessions, or right after a compaction (let the next
/// turn run and report a fresh, smaller figure).
pub async fn latest_input_tokens(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<Option<i64>> {
    let boundary_seq: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(seq), 0)
        FROM events
        WHERE user_id = ? AND session_id = ? AND kind = 'compaction.completed'
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_one(pool)
    .await
    .context("reading latest compaction boundary")?;

    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT payload_json
        FROM events
        WHERE user_id = ?
          AND session_id = ?
          AND kind = 'llm_usage'
          AND seq > ?
        ORDER BY seq DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(boundary_seq)
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

#[allow(clippy::too_many_arguments)]
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
    let session_lock = event_seq_lock(user_id, session_id).await;
    let _guard = session_lock.lock().await;

    let mut last_error: Option<sqlx::Error> = None;
    for attempt in 0..INSERT_MAX_ATTEMPTS {
        match insert_once(
            pool, user_id, session_id, task_id, kind, payload, source, ts,
        )
        .await
        {
            Ok(seq) => return Ok(seq),
            Err(err) if is_retryable_insert_error(&err) && attempt + 1 < INSERT_MAX_ATTEMPTS => {
                let delay = insert_retry_delay(attempt);
                tracing::warn!(
                    user_id,
                    session_id,
                    kind,
                    attempt = attempt + 1,
                    delay_ms = delay.as_millis(),
                    error = %err,
                    "retrying event insert"
                );
                last_error = Some(err);
                sleep(delay).await;
            }
            Err(err) => {
                return Err(err).context("inserting event");
            }
        }
    }

    Err(last_error.expect("event insert retry loop recorded no error")).context("inserting event")
}

#[allow(clippy::too_many_arguments)]
async fn insert_once(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    kind: &str,
    payload: &serde_json::Value,
    source: Option<&str>,
    ts: chrono::DateTime<chrono::Utc>,
) -> std::result::Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE user_id = ? AND session_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await?;

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
    .await?;

    tx.commit().await?;
    Ok(seq)
}

async fn event_seq_lock(user_id: &str, session_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let locks = EVENT_SEQ_LOCKS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
    let key = format!("{user_id}\x1f{session_id}");
    let mut guard = locks.lock().await;
    guard
        .entry(key)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn insert_retry_delay(attempt: usize) -> Duration {
    let multiplier = 1_u64 << attempt.min(5);
    Duration::from_millis((INSERT_RETRY_BASE_MS * multiplier).min(250))
}

fn is_retryable_insert_error(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("database is locked")
        || message.contains("database table is locked")
        || message.contains("busy")
        || message.contains("locked")
        || message.contains("unique constraint failed")
        || message.contains("constraint failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn concurrent_event_inserts_keep_session_seq_dense() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE events (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                task_id TEXT,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                source TEXT,
                ts TEXT NOT NULL,
                persisted_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, seq)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        let user_id = format!("u-{}", uuid::Uuid::new_v4());
        let session_id = format!("s-{}", uuid::Uuid::new_v4());
        let mut joins = Vec::new();
        for i in 0..40 {
            let pool = pool.clone();
            let user_id = user_id.clone();
            let session_id = session_id.clone();
            joins.push(tokio::spawn(async move {
                insert(
                    &pool,
                    &user_id,
                    &session_id,
                    None,
                    "test_event",
                    &serde_json::json!({ "i": i }),
                    Some("test"),
                    chrono::Utc::now(),
                )
                .await
                .unwrap()
            }));
        }

        for join in joins {
            join.await.unwrap();
        }

        let seqs: Vec<i64> = sqlx::query_scalar(
            "SELECT seq FROM events WHERE user_id = ? AND session_id = ? ORDER BY seq",
        )
        .bind(&user_id)
        .bind(&session_id)
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(seqs, (1..=40).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn latest_input_tokens_ignores_pre_compaction_calls() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE events (
                user_id TEXT NOT NULL, session_id TEXT NOT NULL, seq INTEGER NOT NULL,
                task_id TEXT, kind TEXT NOT NULL, payload_json TEXT NOT NULL,
                source TEXT, ts TEXT NOT NULL, persisted_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, seq)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = chrono::Utc::now();
        let ev = |k: &str, p: serde_json::Value| {
            let pool = pool.clone();
            let k = k.to_string();
            async move { insert(&pool, "u", "s", None, &k, &p, Some("t"), now).await.unwrap() }
        };

        // A heavy call, then a compaction → the stale pre-compaction figure must
        // not be returned (else the pre-turn trigger loops).
        ev("llm_usage", serde_json::json!({ "input_tokens": 240_000 })).await;
        ev("compaction.completed", serde_json::json!({ "trigger": "auto_pre_turn" })).await;
        assert_eq!(latest_input_tokens(&pool, "u", "s").await.unwrap(), None);

        // A fresh, smaller call after the boundary is what counts.
        ev("llm_usage", serde_json::json!({ "input_tokens": 15_000 })).await;
        assert_eq!(
            latest_input_tokens(&pool, "u", "s").await.unwrap(),
            Some(15_000)
        );
    }

    #[test]
    fn sqlite_lock_and_unique_errors_are_retryable() {
        assert!(is_retryable_insert_error(&sqlx::Error::Protocol(
            "database is locked".into()
        )));
        assert!(is_retryable_insert_error(&sqlx::Error::Protocol(
            "UNIQUE constraint failed: events.user_id".into()
        )));
        assert!(!is_retryable_insert_error(&sqlx::Error::Protocol(
            "syntax error".into()
        )));
    }
}
