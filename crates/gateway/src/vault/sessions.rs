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

/// Hard-delete a session and every row it owns. The vault has no foreign-key
/// cascades, so we fan out the deletion explicitly inside one transaction.
/// Order matters: rows that reference `tasks` must be deleted before `tasks`,
/// and `tasks` before `sessions`.
pub async fn hard_delete(pool: &SqlitePool, user_id: &str, session_id: &str) -> Result<()> {
    let mut tx = pool.begin().await?;

    // Tables keyed by task_id (no session_id column) — fan out via subquery
    // on `tasks` BEFORE we delete the tasks themselves.
    //
    // R4 fix: `task_metrics` (added in M1.1) has a FK to `tasks(user_id, id)`.
    // Pre-R3 the table was rarely written, so the FK violation on session
    // delete was a latent bug. R3 made every task lifecycle write a
    // metrics row, which means *every* M1-era session would now fail to
    // delete with a SQLite FK constraint error. Add the fan-out delete
    // before `tasks` is purged.
    for sql in [
        "DELETE FROM task_assignments WHERE user_id = ? AND task_id IN \
         (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)",
        "DELETE FROM reasoning_dag_traces WHERE user_id = ? AND task_id IN \
         (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)",
        "DELETE FROM deliverables WHERE user_id = ? AND task_id IN \
         (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)",
        "DELETE FROM reviews WHERE user_id = ? AND deliverable_id IN \
         (SELECT d.id FROM deliverables d JOIN tasks t \
          ON d.user_id = t.user_id AND d.task_id = t.id \
          WHERE t.user_id = ? AND t.session_id = ?)",
        "DELETE FROM task_metrics WHERE user_id = ? AND task_id IN \
         (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)",
    ] {
        sqlx::query(sql)
            .bind(user_id)
            .bind(user_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("deleting task-scoped rows: {sql}"))?;
    }

    // session_compactions has two FK columns to sessions — purge from both.
    sqlx::query(
        "DELETE FROM session_compactions \
         WHERE user_id = ? AND (original_session_id = ? OR compacted_session_id = ?)",
    )
    .bind(user_id)
    .bind(session_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await
    .context("deleting session_compactions rows")?;

    // Tables with a direct session_id column.
    for sql in [
        "DELETE FROM events WHERE user_id = ? AND session_id = ?",
        "DELETE FROM messages WHERE user_id = ? AND session_id = ?",
        "DELETE FROM artifacts WHERE user_id = ? AND session_id = ?",
        "DELETE FROM panels_layouts WHERE user_id = ? AND session_id = ?",
        "DELETE FROM subagent_runs WHERE user_id = ? AND session_id = ?",
        "DELETE FROM tool_call_runs WHERE user_id = ? AND session_id = ?",
        "DELETE FROM decisions WHERE user_id = ? AND session_id = ?",
        "DELETE FROM llm_usage_log WHERE user_id = ? AND session_id = ?",
        "DELETE FROM agent_plan_items WHERE user_id = ? AND session_id = ?",
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
