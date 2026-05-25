use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ToolRunRow {
    pub id: String,
    pub tool_name: String,
    pub arguments_json: String,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub completed_at: Option<String>,
}

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
    let arguments_json = canonicalize_arguments(arguments_json);
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
    .bind(&arguments_json)
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

pub async fn list_recent_for_session(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    limit: i64,
) -> Result<Vec<ToolRunRow>> {
    let rows = sqlx::query_as(
        r#"
        SELECT id, tool_name, arguments_json, result_json, error, completed_at
        FROM tool_call_runs
        WHERE user_id = ? AND session_id = ? AND completed_at IS NOT NULL
        ORDER BY completed_at DESC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing recent tool runs for session")?;
    Ok(rows)
}

pub async fn find_successful_for_session(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    tool_name: &str,
    arguments_json: &str,
) -> Result<Option<ToolRunRow>> {
    let arguments_json = canonicalize_arguments(arguments_json);
    let row = sqlx::query_as(
        r#"
        SELECT id, tool_name, arguments_json, result_json, error, completed_at
        FROM tool_call_runs
        WHERE user_id = ?
          AND session_id = ?
          AND tool_name = ?
          AND arguments_json = ?
          AND success = 1
          AND completed_at IS NOT NULL
        ORDER BY completed_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(tool_name)
    .bind(&arguments_json)
    .fetch_optional(pool)
    .await
    .context("finding successful tool run for session")?;
    Ok(row)
}

pub fn canonicalize_arguments(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "{}".to_string();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return trimmed.to_string();
    };
    normalize_json(value).to_string()
}

fn normalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(normalize_json).collect())
        }
        serde_json::Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                if let Some(value) = map.get(&key) {
                    sorted.insert(key, normalize_json(value.clone()));
                }
            }
            serde_json::Value::Object(sorted)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_arguments_sorts_object_keys_recursively() {
        let raw = r#"{"b":2,"a":{"d":4,"c":3},"e":[{"z":1,"y":2}]}"#;
        assert_eq!(
            canonicalize_arguments(raw),
            r#"{"a":{"c":3,"d":4},"b":2,"e":[{"y":2,"z":1}]}"#
        );
    }

    #[test]
    fn canonicalize_arguments_uses_empty_object_for_empty_input() {
        assert_eq!(canonicalize_arguments(" \n "), "{}");
    }

    #[test]
    fn canonicalize_arguments_keeps_invalid_json_as_trimmed_string() {
        assert_eq!(canonicalize_arguments("  {bad json  "), "{bad json");
    }
}
