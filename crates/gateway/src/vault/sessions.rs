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

pub async fn rename_if_untitled(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    new_title: &str,
) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE sessions \
         SET title = ?, last_active_at = ? \
         WHERE user_id = ? AND id = ? \
           AND (title IS NULL OR trim(title) = '' OR lower(trim(title)) = 'untitled session')",
    )
    .bind(new_title)
    .bind(&now)
    .bind(user_id)
    .bind(session_id)
    .execute(pool)
    .await
    .context("auto-renaming untitled session")?;
    Ok(result.rows_affected() > 0)
}

pub async fn touch(pool: &SqlitePool, user_id: &str, session_id: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE sessions SET last_active_at = ? WHERE user_id = ? AND id = ?")
        .bind(&now)
        .bind(user_id)
        .bind(session_id)
        .execute(pool)
        .await
        .context("touching session last_active_at")?;
    Ok(())
}

/// Hard-delete a session and every row it owns. The vault has no foreign-key
/// cascades, so deletion fans out explicitly inside one transaction.
pub async fn hard_delete(pool: &SqlitePool, user_id: &str, session_id: &str) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "CREATE TEMP TABLE IF NOT EXISTS session_delete_deliverables (id TEXT PRIMARY KEY)",
    )
    .execute(&mut *tx)
    .await
    .context("creating hard-delete deliverable scratch table")?;
    sqlx::query("DELETE FROM session_delete_deliverables")
        .execute(&mut *tx)
        .await
        .context("clearing hard-delete deliverable scratch table")?;
    sqlx::query(
        "INSERT OR IGNORE INTO session_delete_deliverables (id) \
         SELECT id FROM deliverables \
         WHERE user_id = ? AND task_id IN (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await
    .context("staging task deliverables for session hard delete")?;
    sqlx::query(
        "INSERT OR IGNORE INTO session_delete_deliverables (id) \
         SELECT deliverable_id FROM decisions WHERE user_id = ? AND session_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await
    .context("staging decision deliverables for session hard delete")?;

    delete_by_session(&mut tx, "reviews", user_id, session_id).await?;
    delete_by_session(&mut tx, "decisions", user_id, session_id).await?;
    delete_by_session(&mut tx, "deliverables", user_id, session_id).await?;
    delete_by_session(&mut tx, "task_assignments", user_id, session_id).await?;
    delete_by_session(&mut tx, "reasoning_dag_traces", user_id, session_id).await?;
    for table in [
        "agent_plan_items",
        "tool_call_runs",
        "subagent_runs",
        "artifacts",
        "panels_layouts",
        "session_compactions",
        "llm_usage_log",
        "events",
        "messages",
        "tasks",
        "sessions",
    ] {
        delete_by_session(&mut tx, table, user_id, session_id).await?;
    }

    sqlx::query("DELETE FROM session_delete_deliverables")
        .execute(&mut *tx)
        .await
        .context("clearing hard-delete deliverable scratch table after delete")?;
    tx.commit().await?;
    Ok(())
}

async fn delete_by_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    table: &str,
    user_id: &str,
    session_id: &str,
) -> Result<()> {
    let sql = match table {
        "reviews" => {
            "DELETE FROM reviews \
             WHERE user_id = ? \
               AND (decision_id IN (SELECT id FROM decisions WHERE user_id = ? AND session_id = ?) \
                    OR deliverable_id IN (SELECT id FROM session_delete_deliverables))"
        }
        "decisions" => "DELETE FROM decisions WHERE user_id = ? AND session_id = ?",
        "deliverables" => {
            "DELETE FROM deliverables \
             WHERE user_id = ? AND id IN (SELECT id FROM session_delete_deliverables)"
        }
        "task_assignments" => {
            "DELETE FROM task_assignments \
             WHERE user_id = ? AND task_id IN (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)"
        }
        "reasoning_dag_traces" => {
            "DELETE FROM reasoning_dag_traces \
             WHERE user_id = ? AND task_id IN (SELECT id FROM tasks WHERE user_id = ? AND session_id = ?)"
        }
        "agent_plan_items" => "DELETE FROM agent_plan_items WHERE user_id = ? AND session_id = ?",
        "tool_call_runs" => "DELETE FROM tool_call_runs WHERE user_id = ? AND session_id = ?",
        "subagent_runs" => "DELETE FROM subagent_runs WHERE user_id = ? AND session_id = ?",
        "artifacts" => "DELETE FROM artifacts WHERE user_id = ? AND session_id = ?",
        "panels_layouts" => "DELETE FROM panels_layouts WHERE user_id = ? AND session_id = ?",
        "session_compactions" => {
            "DELETE FROM session_compactions \
             WHERE user_id = ? AND (original_session_id = ? OR compacted_session_id = ?)"
        }
        "llm_usage_log" => "DELETE FROM llm_usage_log WHERE user_id = ? AND session_id = ?",
        "events" => "DELETE FROM events WHERE user_id = ? AND session_id = ?",
        "messages" => "DELETE FROM messages WHERE user_id = ? AND session_id = ?",
        "tasks" => "DELETE FROM tasks WHERE user_id = ? AND session_id = ?",
        "sessions" => "DELETE FROM sessions WHERE user_id = ? AND id = ?",
        _ => unreachable!("unknown session delete table"),
    };

    let mut query = sqlx::query(sql).bind(user_id);
    query = match table {
        "deliverables" => query,
        "reviews" | "task_assignments" | "reasoning_dag_traces" => {
            query.bind(user_id).bind(session_id)
        }
        "session_compactions" => query.bind(session_id).bind(session_id),
        _ => query.bind(session_id),
    };
    query
        .execute(&mut **tx)
        .await
        .with_context(|| format!("deleting session rows from {table}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        super::super::MIGRATOR.run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO users (user_id, display_name, created_at) VALUES ('u1', 'User', '2026-05-20T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        sqlx::query_scalar(&sql).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn hard_delete_removes_session_owned_rows_and_task_derived_rows() {
        let pool = setup_pool().await;
        let now = "2026-05-20T00:00:00Z";
        for session_id in ["s1", "s2"] {
            sqlx::query(
                "INSERT INTO sessions (user_id, id, title, status, pinned, created_at, last_active_at) \
                 VALUES ('u1', ?, NULL, 'active', 0, ?, ?)",
            )
            .bind(session_id)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        sqlx::query(
            "INSERT INTO tasks (user_id, id, session_id, title, goal, expected_deliverable, created_at) \
             VALUES ('u1', 't1', 's1', 'Task', 'Goal', 'research_brief', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (user_id, session_id, seq, role, content_json, created_at) \
             VALUES ('u1', 's1', 1, 'user', '{}', ?), ('u1', 's2', 1, 'user', '{}', ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO events (user_id, session_id, seq, kind, payload_json, ts, persisted_at) \
             VALUES ('u1', 's1', 1, 'tool_call', '{}', ?, ?), ('u1', 's2', 1, 'tool_call', '{}', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_call_runs (user_id, id, session_id, invoker, tool_name, arguments_json, started_at) \
             VALUES ('u1', 'run1', 's1', 'main_agent', 'web_fetch', '{}', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO agent_plan_items (user_id, session_id, task_id, item_id, seq, step, status, created_at, updated_at) \
             VALUES ('u1', 's1', 't1', 'p1', 1, 'step', 'pending', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO subagent_runs (user_id, id, session_id, task_id, spec_name, scope_json, input_json, spawned_at) \
             VALUES ('u1', 'sub1', 's1', 't1', 'analyst', '{}', '{}', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO artifacts (user_id, id, session_id, kind, payload_json, created_at, updated_at) \
             VALUES ('u1', 'a1', 's1', 'note', '{}', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO panels_layouts (user_id, session_id, layout_json, updated_at) \
             VALUES ('u1', 's1', '{}', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_assignments (user_id, task_id, seq, assignee, assigned_by, created_at) \
             VALUES ('u1', 't1', 1, 'agent', 'user', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO reasoning_dag_traces (user_id, task_id, nodes_json, edges_json, updated_at) \
             VALUES ('u1', 't1', '[]', '[]', ?)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO deliverables (user_id, id, task_id, kind, payload_json, status, created_at) \
             VALUES ('u1', 'd1', 't1', 'brief', '{}', 'ready', ?), \
                    ('u1', 'd2', NULL, 'decision', '{}', 'ready', ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO decisions (user_id, id, deliverable_id, task_id, session_id, ticker, direction, rationale_md, confirmed_at) \
             VALUES ('u1', 'dec1', 'd1', 't1', 's1', 'AAPL', 'buy', 'why', ?), \
                    ('u1', 'dec2', 'd2', NULL, 's1', 'MSFT', 'hold', 'why', ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO reviews (user_id, id, deliverable_id, decision_id, summary_md, created_at) \
             VALUES ('u1', 'r1', 'd1', 'dec1', 'summary', ?), \
                    ('u1', 'r2', 'd2', NULL, 'summary', ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_compactions (user_id, id, original_session_id, compacted_session_id, summary_md, messages_retained, messages_removed, trigger, created_at) \
             VALUES ('u1', 'c1', 's1', 's1', 'summary', 1, 2, 'manual', ?), \
                    ('u1', 'c2', 's2', 's1', 'summary', 1, 2, 'manual', ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO llm_usage_log (user_id, id, provider_name, model, invoker, session_id, input_tokens, output_tokens, duration_ms, success, started_at) \
             VALUES ('u1', 'u-s1', 'codex', 'gpt-5.5', 'main_agent', 's1', 10, 5, 100, 1, ?), \
                    ('u1', 'u-s2', 'codex', 'gpt-5.5', 'main_agent', 's2', 10, 5, 100, 1, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        hard_delete(&pool, "u1", "s1").await.unwrap();

        for table in [
            "tasks",
            "tool_call_runs",
            "agent_plan_items",
            "subagent_runs",
            "artifacts",
            "panels_layouts",
            "task_assignments",
            "reasoning_dag_traces",
            "deliverables",
            "decisions",
            "reviews",
            "session_compactions",
        ] {
            assert_eq!(count(&pool, table).await, 0, "{table}");
        }
        assert_eq!(count(&pool, "sessions").await, 1);
        assert_eq!(count(&pool, "messages").await, 1);
        assert_eq!(count(&pool, "events").await, 1);
        assert_eq!(count(&pool, "llm_usage_log").await, 1);
    }
}
