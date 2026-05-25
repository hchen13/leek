//! Session-level operations.
//!
//! `abort_handler` cancels the in-flight agent reply for the session — the
//! token is signalled, the reply pipeline graceful-exits at the next
//! `select!` point and commits whatever partial text it has accumulated.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{ActiveTask, AppError, AppState};
use crate::agent;
use crate::agent::compact::{
    CompactSupportingContext, CompactToolEvidence, CompactWebSearchActivity,
};
use crate::vault::{
    compactions as vault_compactions, events as vault_events, messages as vault_messages,
    sessions as vault_sessions, tool_runs as vault_tool_runs,
};

const COMPACTION_CONTEXT_SQL_LIMIT: i64 = 24;

pub async fn abort_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> StatusCode {
    let map = state.active_replies.lock().await;
    let Some(task) = map.get(&session_id) else {
        // Nothing in flight; treat as a no-op success.
        return StatusCode::ACCEPTED;
    };
    if !task.user_cancellable {
        // Auto pre-turn compaction — refuse the abort. The user has to wait
        // because skipping it would push the next turn over context.
        tracing::info!(
            session_id,
            "abort rejected: in-flight task is not user-cancellable (auto compaction)"
        );
        return StatusCode::CONFLICT;
    }
    task.token.cancel();
    tracing::info!(session_id, "abort signalled");
    StatusCode::ACCEPTED
}

#[derive(Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct EventsResponse {
    pub items: Vec<vault_events::EventRow>,
}

/// `GET /api/v1/sessions/{id}/events?since=<seq>&limit=<n>` — return durable
/// event log rows for the session in ascending seq order. Used by the
/// frontend Events Timeline panel; the live SSE stream covers real-time
/// delivery, this endpoint covers history / re-load.
pub async fn events_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<EventsResponse>, AppError> {
    let rows =
        vault_events::list_for_session(&state.pool, &state.user_id, &session_id, q.since, q.limit)
            .await?;
    Ok(Json(EventsResponse { items: rows }))
}

#[derive(serde::Serialize)]
pub struct ListSessionsResponse {
    pub items: Vec<vault_sessions::SessionRow>,
}

pub async fn list_handler(
    State(state): State<AppState>,
) -> Result<Json<ListSessionsResponse>, AppError> {
    let items = vault_sessions::list(&state.pool, &state.user_id).await?;
    Ok(Json(ListSessionsResponse { items }))
}

#[derive(Deserialize)]
pub struct CreateSessionBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(serde::Serialize)]
pub struct CreateSessionResponse {
    pub id: String,
}

pub async fn create_handler(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), AppError> {
    let id = body
        .id
        .unwrap_or_else(|| format!("s-{}", uuid::Uuid::new_v4().simple()));
    vault_sessions::ensure_exists(&state.pool, &state.user_id, &id, body.title.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(CreateSessionResponse { id })))
}

#[derive(Deserialize)]
pub struct PatchSessionBody {
    pub title: String,
}

pub async fn patch_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PatchSessionBody>,
) -> Result<StatusCode, AppError> {
    vault_sessions::rename(&state.pool, &state.user_id, &session_id, &body.title).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    // Cancel any in-flight task for this session before yanking the data.
    // (Delete is the user's explicit intent — we cancel even auto-compactions
    // here, since the data they're operating on is about to disappear.)
    {
        let mut map = state.active_replies.lock().await;
        if let Some(task) = map.remove(&session_id) {
            task.token.cancel();
        }
    }
    vault_sessions::hard_delete(&state.pool, &state.user_id, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// =====================================================================
// Compaction
// =====================================================================

#[derive(Deserialize, Default)]
pub struct CompactBody {
    /// `manual` (default) | `auto_pre_turn`. Records intent in
    /// session_compactions.trigger so we can analyze who triggers compaction.
    #[serde(default)]
    pub trigger: Option<String>,
    /// Optional focus topic — passed into the summarizer prompt so it gives
    /// extra detail on this thread and shorter coverage of the rest.
    #[serde(default)]
    pub focus: Option<String>,
}

/// `POST /api/v1/sessions/{id}/compact` — summarize the session's history
/// in-place: inserts a `compaction_summary` message into the same session,
/// freeing context budget for the next turn. The endpoint returns 202
/// immediately; subscribers on the session's SSE stream see
/// `compaction.started` / `compaction.completed` / `compaction.aborted`.
pub async fn compact_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<CompactBody>,
) -> Result<StatusCode, AppError> {
    let trigger = body
        .trigger
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "manual".to_string());
    start_compaction(&state, &session_id, &trigger, body.focus.as_deref())
        .await
        .map_err(AppError)?;
    Ok(StatusCode::ACCEPTED)
}

/// Kick off an in-place compaction on `session_id`. Writes a
/// `compaction_summary` message into the same session and leaves all prior
/// messages intact (visible in UI, excluded from LLM context). Common path
/// used by both manual `POST /compact` and auto-pre-turn trigger.
pub async fn start_compaction(
    state: &AppState,
    session_id: &str,
    trigger: &str,
    focus: Option<&str>,
) -> anyhow::Result<()> {
    {
        let map = state.active_replies.lock().await;
        if let Some(task) = map.get(session_id) {
            if !task.token.is_cancelled() {
                anyhow::bail!("compaction rejected: agent reply in progress; abort or wait first");
            }
        }
    }

    let cancel = CancellationToken::new();
    let user_cancellable = trigger != "auto_pre_turn";
    {
        let mut map = state.active_replies.lock().await;
        map.insert(
            session_id.to_string(),
            ActiveTask {
                token: cancel.clone(),
                user_cancellable,
            },
        );
    }

    agent::publish_and_persist(
        &state.pool,
        &state.user_id,
        session_id,
        None,
        &state.event_bus,
        "compaction.started",
        serde_json::json!({ "trigger": trigger, "focus": focus }),
    )
    .await?;

    let pool = state.pool.clone();
    let user_id = state.user_id.clone();
    let bus = state.event_bus.clone();
    let provider = state.provider.clone();
    let active_replies = state.active_replies.clone();
    let source_session = session_id.to_string();
    let focus_owned = focus.map(|s| s.to_string());
    let trigger_for_task = trigger.to_string();
    let cancel_for_task = cancel.clone();
    let cancel_for_cleanup = cancel.clone();

    tokio::spawn(async move {
        let outcome = run_compaction(
            pool.clone(),
            user_id.clone(),
            source_session.clone(),
            provider,
            focus_owned.as_deref(),
            &trigger_for_task,
            cancel_for_task,
        )
        .await;

        if !cancel_for_cleanup.is_cancelled() {
            active_replies.lock().await.remove(&source_session);
        }

        let (kind, payload) = match outcome {
            Ok(report) => (
                "compaction.completed".to_string(),
                serde_json::json!({
                    "summary_md": report.summary_md,
                    "messages_removed": report.messages_removed,
                    "messages_retained": report.messages_retained,
                    "trigger": trigger_for_task,
                }),
            ),
            Err(err) => {
                tracing::warn!(error = ?err, "compaction failed");
                (
                    "compaction.aborted".to_string(),
                    serde_json::json!({
                        "reason": format!("{err:#}"),
                        "trigger": trigger_for_task,
                    }),
                )
            }
        };

        if let Err(e) =
            agent::publish_and_persist(&pool, &user_id, &source_session, None, &bus, &kind, payload)
                .await
        {
            tracing::error!(error = %e, "failed to publish compaction terminal event");
        }
    });

    Ok(())
}

/// Outcome record for a successful compaction — drives the
/// `compaction.completed` event payload + the session_compactions row.
struct CompactionReport {
    summary_md: String,
    messages_removed: i64,
    messages_retained: i64,
}

async fn run_compaction(
    pool: sqlx::SqlitePool,
    user_id: String,
    source_session: String,
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    focus: Option<&str>,
    trigger: &str,
    cancel: CancellationToken,
) -> anyhow::Result<CompactionReport> {
    // Load only the messages that will go into the transcript: everything
    // after the last compaction_summary (or the full history on first compact).
    let all_history = vault_messages::list(&pool, &user_id, &source_session, None, 1000).await?;
    if all_history.is_empty() {
        anyhow::bail!("source session has no messages to compact");
    }

    // Find the last compaction_summary boundary; compact only the tail.
    let start_idx = all_history
        .iter()
        .rposition(|r| r.role == "compaction_summary")
        .map(|i| i + 1)
        .unwrap_or(0);
    let history = &all_history[start_idx..];
    if history.is_empty() {
        anyhow::bail!("no new messages since last compaction");
    }
    let messages_removed = history.len() as i64;
    let supporting_context =
        load_compaction_supporting_context(&pool, &user_id, &source_session, history).await?;

    let summary = agent::compact::summarize_session(
        provider,
        history,
        Some(&supporting_context),
        focus,
        cancel,
    )
    .await?;

    // Write the summary into the same session as a compaction_summary message.
    vault_messages::insert(
        &pool,
        &user_id,
        &source_session,
        "compaction_summary",
        &serde_json::json!({ "type": "text", "text": summary }),
        None,
    )
    .await?;

    vault_compactions::insert(
        &pool,
        &user_id,
        &source_session,
        &source_session,
        &summary,
        1,
        messages_removed,
        None,
        None,
        trigger,
        focus,
    )
    .await?;

    Ok(CompactionReport {
        summary_md: summary,
        messages_removed,
        messages_retained: 1,
    })
}

async fn load_compaction_supporting_context(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    session_id: &str,
    history: &[vault_messages::MessageRow],
) -> anyhow::Result<CompactSupportingContext> {
    let Some(first) = history.first() else {
        return Ok(CompactSupportingContext::default());
    };
    let Some(last) = history.last() else {
        return Ok(CompactSupportingContext::default());
    };

    let mut tool_rows: Vec<vault_tool_runs::ToolRunRow> = sqlx::query_as(
        r#"
        SELECT id, tool_name, arguments_json, result_json, error, completed_at
        FROM tool_call_runs
        WHERE user_id = ?
          AND session_id = ?
          AND completed_at IS NOT NULL
          AND completed_at >= ?
          AND completed_at <= ?
        ORDER BY completed_at DESC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(&first.created_at)
    .bind(&last.created_at)
    .bind(COMPACTION_CONTEXT_SQL_LIMIT)
    .fetch_all(pool)
    .await?;
    tool_rows.reverse();

    let mut web_rows: Vec<vault_events::EventRow> = sqlx::query_as(
        r#"
        SELECT seq, task_id, kind, payload_json, source, ts
        FROM events
        WHERE user_id = ?
          AND session_id = ?
          AND kind = 'web_search_call'
          AND ts >= ?
          AND ts <= ?
        ORDER BY ts DESC, seq DESC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(&first.created_at)
    .bind(&last.created_at)
    .bind(COMPACTION_CONTEXT_SQL_LIMIT)
    .fetch_all(pool)
    .await?;
    web_rows.reverse();

    Ok(CompactSupportingContext {
        tool_runs: tool_rows
            .into_iter()
            .map(|row| CompactToolEvidence {
                tool_name: row.tool_name,
                arguments_json: row.arguments_json,
                result_json: row.result_json,
                error: row.error,
                completed_at: row.completed_at,
            })
            .collect(),
        web_searches: web_rows
            .into_iter()
            .map(|row| CompactWebSearchActivity {
                ts: row.ts,
                payload_json: row.payload_json,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn load_compaction_supporting_context_keeps_latest_rows_in_chronological_order() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE tool_call_runs (
                user_id TEXT NOT NULL,
                id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                arguments_json TEXT NOT NULL,
                result_json TEXT,
                error TEXT,
                completed_at TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE events (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                task_id TEXT,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                source TEXT,
                ts TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        for i in 0..30 {
            let ts = format!("2026-05-20T00:00:{i:02}Z");
            sqlx::query(
                "INSERT INTO tool_call_runs
                 (user_id, id, session_id, tool_name, arguments_json, result_json, completed_at)
                 VALUES ('u1', ?, 's1', 'web_fetch', ?, ?, ?)",
            )
            .bind(format!("tool-{i}"))
            .bind(serde_json::json!({ "i": i }).to_string())
            .bind(serde_json::json!({ "output": format!("tool output {i}") }).to_string())
            .bind(&ts)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO events
                 (user_id, session_id, seq, kind, payload_json, ts)
                 VALUES ('u1', 's1', ?, 'web_search_call', ?, ?)",
            )
            .bind(i + 1)
            .bind(
                serde_json::json!({
                    "action": "search",
                    "detail": format!("query {i}")
                })
                .to_string(),
            )
            .bind(&ts)
            .execute(&pool)
            .await
            .unwrap();
        }

        let history = vec![
            vault_messages::MessageRow {
                seq: 1,
                task_id: None,
                role: "user".into(),
                content_json: serde_json::json!({ "text": "start" }).to_string(),
                created_at: "2026-05-20T00:00:00Z".into(),
            },
            vault_messages::MessageRow {
                seq: 2,
                task_id: None,
                role: "agent".into(),
                content_json: serde_json::json!({ "text": "end" }).to_string(),
                created_at: "2026-05-20T00:00:59Z".into(),
            },
        ];

        let supporting_context = load_compaction_supporting_context(&pool, "u1", "s1", &history)
            .await
            .unwrap();

        assert_eq!(supporting_context.tool_runs.len(), 24);
        assert_eq!(supporting_context.web_searches.len(), 24);
        assert_eq!(supporting_context.tool_runs[0].arguments_json, r#"{"i":6}"#);
        assert_eq!(
            supporting_context.tool_runs[23].arguments_json,
            r#"{"i":29}"#
        );
        assert!(
            supporting_context.web_searches[0]
                .payload_json
                .contains("query 6")
        );
        assert!(
            supporting_context.web_searches[23]
                .payload_json
                .contains("query 29")
        );
    }
}
