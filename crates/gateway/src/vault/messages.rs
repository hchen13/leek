//! Message row helpers.
//!
//! `messages` doubles as the session's append-only conversation queue. Beyond
//! the user-visible roles (`user` / `agent` / `compaction_summary`), the agent
//! loop persists two tool-trace roles so a turn's tool dialog survives across
//! turns and replays verbatim into the Responses API `input` array:
//! `assistant_tool_calls` (one row per iteration's batch of function_calls) and
//! `tool_result` (one row per function_call_output, keyed by call_id). All
//! roles share the same `(user_id, session_id, seq)` timeline, so call ordering
//! ("call before output") is guaranteed by seq alone — no second table, no role
//! CHECK constraint. `row_to_input_items` turns each row back into raw Responses
//! API items; `list_for_ui` hides the tool-trace roles from the chat transcript.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MessageRow {
    pub seq: i64,
    pub task_id: Option<String>,
    pub role: String,
    pub content_json: String,
    pub created_at: String,
}

pub async fn insert(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    role: &str,
    content_json: &serde_json::Value,
    task_id: Option<&str>,
) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;

    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE user_id = ? AND session_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await
    .context("computing next message seq")?;

    sqlx::query(
        "INSERT INTO messages (user_id, session_id, seq, task_id, role, content_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(session_id)
    .bind(seq)
    .bind(task_id)
    .bind(role)
    .bind(content_json.to_string())
    .bind(&now)
    .execute(&mut *tx)
    .await
    .context("inserting message")?;

    tx.commit().await?;
    Ok(seq)
}

pub async fn list(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    since_seq: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageRow>> {
    let rows: Vec<MessageRow> = sqlx::query_as(
        r#"
        SELECT seq, task_id, role, content_json, created_at
        FROM messages
        WHERE user_id = ? AND session_id = ? AND seq > ?
        ORDER BY seq ASC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(since_seq.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing messages")?;
    Ok(rows)
}

/// UI-facing list: the chat transcript only renders user / agent /
/// compaction_summary bubbles, so the append-only tool-trace roles are filtered
/// out here. The frontend coerces any unknown role to a (blank) user bubble, so
/// these MUST NOT leak into the HTTP messages list.
pub async fn list_for_ui(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    since_seq: Option<i64>,
    limit: i64,
) -> Result<Vec<MessageRow>> {
    let rows: Vec<MessageRow> = sqlx::query_as(
        r#"
        SELECT seq, task_id, role, content_json, created_at
        FROM messages
        WHERE user_id = ? AND session_id = ? AND seq > ?
          AND role IN ('user', 'agent', 'compaction_summary')
        ORDER BY seq ASC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(since_seq.unwrap_or(0))
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing messages for ui")?;
    Ok(rows)
}

/// Reconstruct a stored conversation-queue row into raw Responses API `input`
/// items. Returns a Vec because an `assistant_tool_calls` row fans out into one
/// `function_call` item per batched call. `compaction_summary` returns nothing
/// (the summary is injected separately as runtime context), as does any
/// unrecognized role or malformed payload.
pub fn row_to_input_items(role: &str, content_json: &str) -> Vec<Value> {
    let v: Value = match serde_json::from_str(content_json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    match role {
        "user" => vec![json!({
            "role": "user",
            "content": v.get("text").and_then(|t| t.as_str()).unwrap_or(""),
        })],
        "agent" => vec![json!({
            "role": "assistant",
            "content": v.get("text").and_then(|t| t.as_str()).unwrap_or(""),
        })],
        "assistant_tool_calls" => v
            .get("items")
            .and_then(|i| i.as_array())
            .map(|arr| arr.to_vec())
            .unwrap_or_default(),
        "tool_result" => vec![v],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                task_id TEXT,
                role TEXT NOT NULL,
                content_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, seq)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn tool_trace_round_trips_through_queue() {
        let pool = mem_pool().await;
        insert(
            &pool,
            "u1",
            "s1",
            "user",
            &json!({ "type": "text", "text": "查一下茅台" }),
            None,
        )
        .await
        .unwrap();
        insert(
            &pool,
            "u1",
            "s1",
            "assistant_tool_calls",
            &json!({
                "type": "tool_calls",
                "items": [
                    {"type":"function_call","call_id":"c1","name":"get_financials","arguments":"{\"ts_code\":\"600519\"}"},
                    {"type":"function_call","call_id":"c2","name":"get_company_info","arguments":"{}"}
                ]
            }),
            None,
        )
        .await
        .unwrap();
        insert(
            &pool,
            "u1",
            "s1",
            "tool_result",
            &json!({"type":"function_call_output","call_id":"c1","output":"营收 1700 亿"}),
            None,
        )
        .await
        .unwrap();
        insert(
            &pool,
            "u1",
            "s1",
            "tool_result",
            &json!({"type":"function_call_output","call_id":"c2","output":"贵州茅台股份有限公司"}),
            None,
        )
        .await
        .unwrap();

        let all = list(&pool, "u1", "s1", None, 100).await.unwrap();
        let replay: Vec<Value> = all
            .iter()
            .flat_map(|r| row_to_input_items(&r.role, &r.content_json))
            .collect();

        // user msg + 2 function_call + 2 function_call_output
        assert_eq!(replay.len(), 5);
        assert_eq!(replay[0]["role"], "user");
        assert_eq!(replay[1]["type"], "function_call");
        assert_eq!(replay[1]["call_id"], "c1");
        assert_eq!(replay[1]["arguments"], "{\"ts_code\":\"600519\"}");
        assert_eq!(replay[2]["type"], "function_call");
        assert_eq!(replay[2]["call_id"], "c2");
        assert_eq!(replay[3]["type"], "function_call_output");
        assert_eq!(replay[3]["call_id"], "c1");
        assert_eq!(replay[3]["output"], "营收 1700 亿");
        assert_eq!(replay[4]["call_id"], "c2");
    }

    #[tokio::test]
    async fn list_for_ui_hides_tool_trace_roles() {
        let pool = mem_pool().await;
        for (role, body) in [
            ("user", json!({"type":"text","text":"hi"})),
            (
                "assistant_tool_calls",
                json!({"type":"tool_calls","items":[]}),
            ),
            (
                "tool_result",
                json!({"type":"function_call_output","call_id":"c1","output":"x"}),
            ),
            ("agent", json!({"type":"text","text":"answer"})),
        ] {
            insert(&pool, "u1", "s1", role, &body, None).await.unwrap();
        }
        let ui = list_for_ui(&pool, "u1", "s1", None, 100).await.unwrap();
        let roles: Vec<&str> = ui.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "agent"]);
    }

    #[test]
    fn row_to_input_items_skips_compaction_and_malformed() {
        assert!(row_to_input_items("compaction_summary", r#"{"text":"s"}"#).is_empty());
        assert!(row_to_input_items("user", "not-json").is_empty());
        assert!(row_to_input_items("totally_unknown", r#"{"text":"x"}"#).is_empty());
    }
}
