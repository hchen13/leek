use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;

#[allow(clippy::too_many_arguments)]
pub async fn start(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    session_id: &str,
    task_id: Option<&str>,
    invoker: &str,
    tool_name: &str,
    arguments_json: &str,
) -> Result<String> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO tool_call_runs
          (user_id, id, session_id, task_id, invoker, tool_name, arguments_json, started_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(id)
    .bind(session_id)
    .bind(task_id)
    .bind(invoker)
    .bind(tool_name)
    .bind(arguments_json)
    .bind(&now)
    .execute(pool)
    .await
    .context("inserting tool_call_runs row")?;
    Ok(now)
}

pub async fn finish(
    pool: &SqlitePool,
    user_id: &str,
    id: &str,
    result_json: Option<&serde_json::Value>,
    success: bool,
    error: Option<&str>,
    duration_ms: i64,
) -> Result<()> {
    let completed_at = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE tool_call_runs
        SET result_json = ?, success = ?, error = ?, duration_ms = ?, completed_at = ?
        WHERE user_id = ? AND id = ?
        "#,
    )
    .bind(result_json.map(serde_json::Value::to_string))
    .bind(if success { 1 } else { 0 })
    .bind(error)
    .bind(duration_ms)
    .bind(&completed_at)
    .bind(user_id)
    .bind(id)
    .execute(pool)
    .await
    .context("updating tool_call_runs row")?;
    Ok(())
}
