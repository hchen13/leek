//! Message row helpers.
//!
//! `messages` doubles as the session's append-only conversation queue. Beyond
//! the user-visible roles (`user` / `agent` / `compaction_summary`), the agent
//! loop persists hidden model/tool-trace roles so a turn's tool dialog survives
//! across turns and replays verbatim into the Responses API `input` array:
//! `assistant_reasoning` (encrypted reasoning output items),
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
    // `messages` is an append-only queue whose seq is `MAX(seq)+1` per session,
    // like `events`. Concurrent writers (multiple sessions, or a turn's tool
    // traces racing another session) collide on the single SQLite write lock or
    // the seq PK; busy_timeout alone does not cover SQLITE_BUSY_SNAPSHOT, so we
    // retry the whole transaction — same policy as the events writer.
    use crate::vault::events::{
        insert_retry_delay, is_retryable_insert_error, INSERT_MAX_ATTEMPTS,
    };

    let mut last_error: Option<sqlx::Error> = None;
    for attempt in 0..INSERT_MAX_ATTEMPTS {
        match insert_once(pool, user_id, session_id, role, content_json, task_id).await {
            Ok(seq) => return Ok(seq),
            Err(err) if is_retryable_insert_error(&err) && attempt + 1 < INSERT_MAX_ATTEMPTS => {
                last_error = Some(err);
                tokio::time::sleep(insert_retry_delay(attempt)).await;
            }
            Err(err) => return Err(err).context("inserting message"),
        }
    }
    Err(last_error.expect("message insert retry loop recorded no error"))
        .context("inserting message")
}

async fn insert_once(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    role: &str,
    content_json: &serde_json::Value,
    task_id: Option<&str>,
) -> std::result::Result<i64, sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;

    let seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE user_id = ? AND session_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await?;

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
    .await?;

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
        "assistant_reasoning" => v
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
            "assistant_reasoning",
            &json!({
                "type": "reasoning_items",
                "items": [
                    {"type":"reasoning","summary":[],"encrypted_content":"ENC1"}
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

        // user msg + reasoning + 2 function_call + 2 function_call_output
        assert_eq!(replay.len(), 6);
        assert_eq!(replay[0]["role"], "user");
        assert_eq!(replay[1]["type"], "reasoning");
        assert_eq!(replay[1]["encrypted_content"], "ENC1");
        assert_eq!(replay[2]["type"], "function_call");
        assert_eq!(replay[2]["call_id"], "c1");
        assert_eq!(replay[2]["arguments"], "{\"ts_code\":\"600519\"}");
        assert_eq!(replay[3]["type"], "function_call");
        assert_eq!(replay[3]["call_id"], "c2");
        assert_eq!(replay[4]["type"], "function_call_output");
        assert_eq!(replay[4]["call_id"], "c1");
        assert_eq!(replay[4]["output"], "营收 1700 亿");
        assert_eq!(replay[5]["call_id"], "c2");
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
                "assistant_reasoning",
                json!({"type":"reasoning_items","items":[]}),
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

    #[tokio::test]
    async fn concurrent_inserts_retry_and_keep_seq_dense() {
        // Real file DB with the production pragmas (WAL + busy_timeout) so the
        // contention matches production: concurrent writers hit SQLITE_BUSY /
        // a seq PK race, which busy_timeout + the retry loop resolve. This is
        // the exact race that 500'd a message insert before the retry existed.
        use sqlx::sqlite::{
            SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
        };
        use std::str::FromStr;
        use std::time::Duration;

        let path = std::env::temp_dir().join(format!("leek_msg_{}.db", uuid::Uuid::new_v4()));
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
                user_id TEXT NOT NULL, session_id TEXT NOT NULL, seq INTEGER NOT NULL,
                task_id TEXT, role TEXT NOT NULL, content_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, seq)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Production shape: many sessions active at once (cross-session write
        // contention), each session writing its own messages sequentially —
        // mirrors several POSTs / turns racing on the single SQLite write lock.
        let mut joins = Vec::new();
        for s in 0..8 {
            let pool = pool.clone();
            joins.push(tokio::spawn(async move {
                let sid = format!("s{s}");
                for i in 0..3 {
                    insert(&pool, "u", &sid, "user", &json!({ "text": i }), None)
                        .await
                        .unwrap();
                }
            }));
        }
        for j in joins {
            j.await.unwrap();
        }

        for s in 0..8 {
            let (sid, n) = (format!("s{s}"), 3);
            let seqs: Vec<i64> = sqlx::query_scalar(
                "SELECT seq FROM messages WHERE user_id='u' AND session_id=? ORDER BY seq",
            )
            .bind(&sid)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                seqs,
                (1..=n).collect::<Vec<_>>(),
                "session {sid} seqs must be dense"
            );
        }

        pool.close().await;
        let p = path.display().to_string();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{p}-wal"));
        let _ = std::fs::remove_file(format!("{p}-shm"));
    }

    #[test]
    fn row_to_input_items_skips_compaction_and_malformed() {
        assert!(row_to_input_items("compaction_summary", r#"{"text":"s"}"#).is_empty());
        assert!(row_to_input_items("user", "not-json").is_empty());
        assert!(row_to_input_items("totally_unknown", r#"{"text":"x"}"#).is_empty());
    }
}
