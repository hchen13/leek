//! Task row helpers.
//!
//! P1 slice: routing layer creates `in_progress` tasks directly (skipping
//! `queued` — that path is for cron / agent_proposed). Lifecycle simplified
//! to `in_progress` → `delivered`; awaiting_user / confirmed / rejected /
//! cancelled lifecycle states land when we wire the full main agent loop.

use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewTask<'a> {
    pub title: &'a str,
    pub goal: &'a str,
    pub expected_deliverable: &'a str,
    pub source: &'a str,           // "user" / "cron" / "agent_proposed"
    pub constraints_json: Option<&'a str>,
    pub context_refs_json: Option<&'a str>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)] // additional fields populated by sqlx; consumed once UI surfaces them
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub expected_deliverable: String,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub delivered_at: Option<String>,
}

pub async fn insert_in_progress(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    new_task: NewTask<'_>,
) -> Result<String> {
    let task_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();

    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO tasks
          (user_id, id, session_id, title, goal, constraints_json,
           expected_deliverable, priority, context_refs_json, source,
           status, created_at, started_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, 'normal', ?, ?, 'in_progress', ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(&task_id)
    .bind(session_id)
    .bind(new_task.title)
    .bind(new_task.goal)
    .bind(new_task.constraints_json)
    .bind(new_task.expected_deliverable)
    .bind(new_task.context_refs_json)
    .bind(new_task.source)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await
    .context("inserting task row")?;

    sqlx::query(
        r#"
        INSERT INTO task_assignments
          (user_id, task_id, seq, assignee, assigned_by, reason, created_at)
        VALUES (?, ?, 1, 'main_agent', ?, NULL, ?)
        "#,
    )
    .bind(user_id)
    .bind(&task_id)
    .bind(new_task.source)
    .bind(&now)
    .execute(&mut *tx)
    .await
    .context("inserting task_assignment row")?;

    tx.commit().await?;
    Ok(task_id)
}

#[allow(dead_code)] // used once the routing layer can attach to existing in-progress task
pub async fn get_active_for_session(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
) -> Result<Option<TaskRow>> {
    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT id, title, goal, expected_deliverable, status,
               created_at, started_at, delivered_at
        FROM tasks
        WHERE user_id = ? AND session_id = ?
          AND status IN ('in_progress', 'awaiting_user')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await
    .context("looking up active task")?;
    Ok(row)
}

pub async fn mark_delivered(
    pool: &SqlitePool,
    user_id: &str,
    task_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE tasks
        SET status = 'delivered', delivered_at = ?
        WHERE user_id = ? AND id = ? AND status = 'in_progress'
        "#,
    )
    .bind(&now)
    .bind(user_id)
    .bind(task_id)
    .execute(pool)
    .await
    .context("marking task delivered")?;
    Ok(())
}

/// Update the `task_id` column for a message that was inserted before its
/// task existed (POST handler writes user message first, then routing decides).
pub async fn link_message(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    message_seq: i64,
    task_id: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE messages SET task_id = ? \
         WHERE user_id = ? AND session_id = ? AND seq = ?",
    )
    .bind(task_id)
    .bind(user_id)
    .bind(session_id)
    .bind(message_seq)
    .execute(pool)
    .await
    .context("linking message to task")?;
    Ok(())
}

/// Write a finished deliverable row (P1 simplification: status starts as
/// `ready`; we don't model the draft → ready transition for the chat_reply slice).
pub async fn write_deliverable(
    pool: &SqlitePool,
    user_id: &str,
    task_id: &str,
    kind: &str,
    text: &str,
) -> Result<String> {
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO deliverables
          (user_id, id, task_id, kind, payload_json, status, created_at, ready_at)
        VALUES (?, ?, ?, ?, ?, 'ready', ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(&id)
    .bind(task_id)
    .bind(kind)
    .bind(serde_json::json!({ "text": text }).to_string())
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .context("inserting deliverable row")?;
    Ok(id)
}
