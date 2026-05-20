//! Raw LLM-layer transcripts — the F2 archive.
//!
//! Every codex-backend call (main agent iteration + auto-compaction summary
//! call) writes one row here. The processed `events` table is the
//! user-visible record of a turn; this is the engineering log behind it,
//! so a debug task asking "what did the provider actually send / receive?"
//! reads the vault instead of replaying against a live backend.
//!
//! Lifecycle of a row:
//!
//! 1. `insert_request` — called just before the HTTP POST. The row's
//!    `response_stream` / `http_status` / `finished_at` are NULL at this
//!    point; a gateway crash mid-stream still leaves the request body in
//!    the vault.
//! 2. `finalize` — called when the SSE stream ends (naturally, or because
//!    its consumer dropped it, or because the HTTP response was a non-2xx
//!    that we never streamed). Fills in the response bytes, the HTTP
//!    status, and the finish timestamp. The `(turn_id, iteration)` UNIQUE
//!    constraint makes a double finalize a hard error rather than silent
//!    data loss.

use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqlitePool};

/// One row's metadata, with body sizes substituted for the bodies
/// themselves. The list endpoint returns these — the bodies travel via
/// the dedicated raw-byte endpoints.
#[derive(Clone, Serialize)]
pub struct TranscriptMeta {
    pub id: i64,
    pub turn_id: String,
    pub iteration: i64,
    pub model: String,
    pub http_status: Option<i64>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Bytes of `request_body`. Always non-zero (the row exists).
    pub request_bytes: i64,
    /// Bytes of `response_stream`. NULL means never finalized.
    pub response_bytes: Option<i64>,
}

/// Insert the request side of a transcript row. Returns the row id used by
/// `finalize`. Called just before the codex-backend HTTP POST, so a crash
/// between this insert and `finalize` still leaves the request behind.
pub async fn insert_request(
    pool: &SqlitePool,
    session_id: &str,
    turn_id: &str,
    iteration: i64,
    model: &str,
    request_body: &[u8],
    started_at: &str,
) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO llm_transcripts \
           (session_id, turn_id, iteration, model, request_body, started_at) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(iteration)
    .bind(model)
    .bind(request_body)
    .bind(started_at)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get::<i64, _>("id")?)
}

/// Fill in the response side. Called once, when the SSE stream ends or
/// when the request never streamed (non-2xx).
///
/// `http_status` conventions: the actual HTTP code on a 2xx (`200`) or
/// 4xx/5xx; `0` on a transport-level mid-stream failure (the response
/// body is whatever bytes had arrived).
pub async fn finalize(
    pool: &SqlitePool,
    row_id: i64,
    response_stream: &[u8],
    http_status: i64,
    finished_at: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE llm_transcripts \
            SET response_stream = ?, http_status = ?, finished_at = ? \
          WHERE id = ?",
    )
    .bind(response_stream)
    .bind(http_status)
    .bind(finished_at)
    .bind(row_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// List a session's transcripts in chronological (`turn_id`, `iteration`)
/// order. Bodies are summarized as sizes; the raw bytes are served by the
/// per-row endpoints.
pub async fn list_by_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<TranscriptMeta>> {
    let rows = sqlx::query(
        "SELECT id, turn_id, iteration, model, http_status, \
                started_at, finished_at, \
                length(request_body)    AS request_bytes, \
                length(response_stream) AS response_bytes \
           FROM llm_transcripts \
          WHERE session_id = ? \
          ORDER BY turn_id ASC, iteration ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok(TranscriptMeta {
                id: row.try_get("id")?,
                turn_id: row.try_get("turn_id")?,
                iteration: row.try_get("iteration")?,
                model: row.try_get("model")?,
                http_status: row.try_get("http_status")?,
                started_at: row.try_get("started_at")?,
                finished_at: row.try_get("finished_at")?,
                request_bytes: row.try_get("request_bytes")?,
                response_bytes: row.try_get("response_bytes")?,
            })
        })
        .collect()
}

/// Raw `request_body` bytes for one transcript row, scoped to a session
/// (mismatched session → `None`, so a leaked turn_id from another session
/// can't be probed). Returns `None` if the row doesn't exist.
pub async fn get_request_body(
    pool: &SqlitePool,
    session_id: &str,
    turn_id: &str,
    iteration: i64,
) -> Result<Option<Vec<u8>>> {
    let row = sqlx::query(
        "SELECT request_body FROM llm_transcripts \
          WHERE session_id = ? AND turn_id = ? AND iteration = ?",
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(iteration)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.try_get::<Vec<u8>, _>("request_body")).transpose()?)
}

/// Raw `response_stream` bytes for one transcript row. `Ok(None)` if the
/// row doesn't exist or doesn't match the session; `Ok(Some(empty))` if
/// the row exists but was never finalized (gateway crash / pending).
pub async fn get_response_stream(
    pool: &SqlitePool,
    session_id: &str,
    turn_id: &str,
    iteration: i64,
) -> Result<Option<Vec<u8>>> {
    let row = sqlx::query(
        "SELECT response_stream FROM llm_transcripts \
          WHERE session_id = ? AND turn_id = ? AND iteration = ?",
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(iteration)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let bytes: Option<Vec<u8>> = row.try_get("response_stream")?;
    Ok(Some(bytes.unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;

    /// Helper: open a fresh vault on a unique temp-file path (uuid-named so
    /// parallel tests don't collide), insert one session, return the pool.
    /// The file is left behind for the OS's temp cleanup — there is no
    /// tempfile dev-dep in this workspace.
    async fn fresh_pool() -> SqlitePool {
        let path = std::env::temp_dir().join(format!(
            "leek-transcripts-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let vault = Vault::open(&path).await.unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, title, created_at, last_active_at) \
             VALUES (?, ?, NULL, ?, ?)",
        )
        .bind("s1")
        .bind(crate::vault::LOCAL_USER)
        .bind("2026-05-20T00:00:00Z")
        .bind("2026-05-20T00:00:00Z")
        .execute(&vault.pool)
        .await
        .unwrap();
        vault.pool
    }

    #[tokio::test]
    async fn insert_and_finalize_round_trip() {
        let pool = fresh_pool().await;
        let row_id = insert_request(
            &pool,
            "s1",
            "t-1",
            1,
            "gpt-5.5",
            b"{\"model\":\"gpt-5.5\"}",
            "2026-05-20T00:00:00Z",
        )
        .await
        .unwrap();

        finalize(
            &pool,
            row_id,
            b"data: {\"type\":\"x\"}\n\ndata: [DONE]\n\n",
            200,
            "2026-05-20T00:00:01Z",
        )
        .await
        .unwrap();

        let list = list_by_session(&pool, "s1").await.unwrap();
        assert_eq!(list.len(), 1);
        let m = &list[0];
        assert_eq!(m.turn_id, "t-1");
        assert_eq!(m.iteration, 1);
        assert_eq!(m.model, "gpt-5.5");
        assert_eq!(m.http_status, Some(200));
        assert_eq!(m.request_bytes, b"{\"model\":\"gpt-5.5\"}".len() as i64);
        assert!(m.response_bytes.unwrap() > 0);

        let req = get_request_body(&pool, "s1", "t-1", 1).await.unwrap().unwrap();
        assert_eq!(req, b"{\"model\":\"gpt-5.5\"}");

        let resp = get_response_stream(&pool, "s1", "t-1", 1).await.unwrap().unwrap();
        assert!(std::str::from_utf8(&resp).unwrap().contains("[DONE]"));
    }

    #[tokio::test]
    async fn turn_iteration_uniqueness() {
        let pool = fresh_pool().await;
        insert_request(&pool, "s1", "t-1", 1, "m", b"a", "2026-05-20T00:00:00Z")
            .await
            .unwrap();
        // Duplicate (turn_id, iteration) — UNIQUE rejects it.
        let dup = insert_request(&pool, "s1", "t-1", 1, "m", b"b", "2026-05-20T00:00:00Z").await;
        assert!(dup.is_err(), "expected UNIQUE violation, got {dup:?}");
    }

    #[tokio::test]
    async fn list_orders_by_turn_then_iteration() {
        let pool = fresh_pool().await;
        // Insert out of order to confirm the SQL `ORDER BY` does the work.
        insert_request(&pool, "s1", "t-2", 1, "m", b"q", "t2").await.unwrap();
        insert_request(&pool, "s1", "t-1", 2, "m", b"q", "t12").await.unwrap();
        insert_request(&pool, "s1", "t-1", 1, "m", b"q", "t11").await.unwrap();
        let list = list_by_session(&pool, "s1").await.unwrap();
        let keys: Vec<(&str, i64)> = list.iter().map(|m| (m.turn_id.as_str(), m.iteration)).collect();
        assert_eq!(keys, vec![("t-1", 1), ("t-1", 2), ("t-2", 1)]);
    }

    #[tokio::test]
    async fn get_endpoints_scope_to_session() {
        let pool = fresh_pool().await;
        // Add a second session and a transcript on it.
        sqlx::query(
            "INSERT INTO sessions (id, user_id, title, created_at, last_active_at) \
             VALUES (?, ?, NULL, ?, ?)",
        )
        .bind("s2")
        .bind(crate::vault::LOCAL_USER)
        .bind("2026-05-20T00:00:00Z")
        .bind("2026-05-20T00:00:00Z")
        .execute(&pool)
        .await
        .unwrap();
        insert_request(&pool, "s2", "t-1", 1, "m", b"belongs to s2", "ts").await.unwrap();

        // Querying s1's view of the SAME (turn_id, iteration) returns None.
        let cross = get_request_body(&pool, "s1", "t-1", 1).await.unwrap();
        assert!(cross.is_none());

        // And s2's own view returns the bytes.
        let own = get_request_body(&pool, "s2", "t-1", 1).await.unwrap().unwrap();
        assert_eq!(own, b"belongs to s2");
    }

    #[tokio::test]
    async fn unfinalized_row_response_is_empty() {
        let pool = fresh_pool().await;
        insert_request(&pool, "s1", "t-1", 1, "m", b"req", "ts")
            .await
            .unwrap();
        // Never finalized — body endpoint returns Some(empty), not None.
        let resp = get_response_stream(&pool, "s1", "t-1", 1).await.unwrap().unwrap();
        assert!(resp.is_empty());

        let list = list_by_session(&pool, "s1").await.unwrap();
        assert_eq!(list[0].response_bytes, None);
        assert_eq!(list[0].http_status, None);
        assert_eq!(list[0].finished_at, None);
    }
}
