//! Message rows. `seq` is per-session monotonic.
//!
//! M4.1.7 added the `tool_dialog` column to persist a turn's
//! cumulative Codex Responses-API input items (`function_call` +
//! `function_call_output`). drive() at turn finalize writes the
//! assistant row with the turn's full tool dialog serialized as a JSON
//! array; drive() at the next turn's start re-hydrates a sliding
//! window of recent assistant rows' dialogs so the model sees what its
//! prior turns called and what came back — instead of starting blind.

use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Serialize, sqlx::FromRow)]
pub struct Message {
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
    /// M4.1.7: JSON array of `additional_inputs` items the turn ending
    /// in this assistant row accumulated. NULL for user rows, for the
    /// system-emitted blocked-by-hook assistant row (no tools ran),
    /// and for pre-M4.1.7 historical rows.
    pub tool_dialog: Option<String>,
}

/// Append a message to a session. The per-session `seq` is allocated
/// atomically (`MAX(seq) + 1`) inside the insert. `tool_dialog` is
/// stored as NULL — use [`insert_with_tool_dialog`] for assistant
/// rows that ran tools.
pub async fn insert(
    pool: &SqlitePool,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<Message> {
    insert_with_tool_dialog(pool, session_id, role, content, None).await
}

/// Append a message and optionally attach the turn's serialized
/// `additional_inputs` JSON array. Pre-M4.1.7 callers go through
/// [`insert`] which sends `None`.
pub async fn insert_with_tool_dialog(
    pool: &SqlitePool,
    session_id: &str,
    role: &str,
    content: &str,
    tool_dialog: Option<&str>,
) -> Result<Message> {
    let now = chrono::Utc::now().to_rfc3339();
    let msg = sqlx::query_as::<_, Message>(
        "INSERT INTO messages (session_id, seq, role, content, created_at, tool_dialog) \
         VALUES (?, (SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?), ?, ?, ?, ?) \
         RETURNING seq, role, content, created_at, tool_dialog",
    )
    .bind(session_id)
    .bind(session_id)
    .bind(role)
    .bind(content)
    .bind(&now)
    .bind(tool_dialog)
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
        "SELECT seq, role, content, created_at, tool_dialog FROM messages \
         WHERE session_id = ? AND seq > ? ORDER BY seq ASC LIMIT ?",
    )
    .bind(session_id)
    .bind(since.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{sessions, LOCAL_USER};

    async fn fresh_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        // Seed `local` user — sessions.user_id has a FK to users(id).
        // Production code does this in `Vault::open`; tests use an in-mem
        // pool directly so we replicate the seed inline.
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT OR IGNORE INTO users (id, created_at) VALUES (?, ?)")
            .bind(LOCAL_USER)
            .bind(&now)
            .execute(&pool)
            .await
            .unwrap();
        sessions::ensure(&pool, "sess-1").await.unwrap();
        pool
    }

    #[tokio::test]
    async fn round_trip_tool_dialog_none() {
        // M4.1.7: legacy insert() leaves tool_dialog NULL; list reads it back.
        let pool = fresh_pool().await;
        let m = insert(&pool, "sess-1", "user", "hi").await.unwrap();
        assert_eq!(m.tool_dialog, None);
        let rows = list(&pool, "sess-1", None, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool_dialog, None);
    }

    #[tokio::test]
    async fn round_trip_tool_dialog_some() {
        // M4.1.7: insert_with_tool_dialog stores JSON; list returns same.
        let pool = fresh_pool().await;
        let dialog =
            r#"[{"type":"function_call","name":"market_overview","arguments":"{}"}]"#;
        let m = insert_with_tool_dialog(&pool, "sess-1", "assistant", "ok", Some(dialog))
            .await
            .unwrap();
        assert_eq!(m.tool_dialog.as_deref(), Some(dialog));
        let rows = list(&pool, "sess-1", None, 10).await.unwrap();
        assert_eq!(rows[0].tool_dialog.as_deref(), Some(dialog));
    }
}
