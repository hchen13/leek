use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

pub struct SubagentStart<'a> {
    pub session_id: &'a str,
    pub task_id: &'a str,
    pub spec_name: &'a str,
    pub scope_json: &'a serde_json::Value,
    pub input_json: &'a serde_json::Value,
}

pub async fn start(pool: &SqlitePool, user_id: &str, input: SubagentStart<'_>) -> Result<String> {
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO subagent_runs
          (user_id, id, session_id, task_id, spec_name, scope_json, input_json, spawned_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(&id)
    .bind(input.session_id)
    .bind(input.task_id)
    .bind(input.spec_name)
    .bind(input.scope_json.to_string())
    .bind(input.input_json.to_string())
    .bind(&now)
    .execute(pool)
    .await
    .context("inserting subagent_runs row")?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn finish(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    output_json: Option<&serde_json::Value>,
    success: bool,
    error: Option<&str>,
    tokens_used: i64,
    turns: i64,
    duration_ms: i64,
) -> Result<()> {
    let completed_at = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE subagent_runs
        SET output_json = ?, success = ?, error = ?, tokens_used = ?,
            turns = ?, duration_ms = ?, completed_at = ?
        WHERE user_id = ? AND id = ?
        "#,
    )
    .bind(output_json.map(serde_json::Value::to_string))
    .bind(if success { 1 } else { 0 })
    .bind(error)
    .bind(tokens_used)
    .bind(turns)
    .bind(duration_ms)
    .bind(&completed_at)
    .bind(user_id)
    .bind(id)
    .execute(pool)
    .await
    .context("updating subagent_runs row")?;
    Ok(())
}
