//! Session-level operations.
//!
//! `abort_handler` cancels the in-flight agent reply for the session — the
//! token is signalled, the reply pipeline graceful-exits at the next
//! `select!` point and commits whatever partial text it has accumulated.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{ActiveTask, AppError, AppState};
use crate::agent;
use crate::vault::{
    compactions as vault_compactions, events as vault_events, messages as vault_messages,
    sessions as vault_sessions,
};

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

/// A session row plus its live runtime state. `running` is true while an
/// agent reply (or compaction) is in flight for the session — derived from
/// `active_replies`, the same map `abort_handler` consults. The frontend uses
/// it to (a) show a live dot in the session list and (b) re-attach the
/// "thinking…" UI when you switch back into a session whose reply is still
/// streaming.
#[derive(serde::Serialize)]
pub struct SessionListItem {
    #[serde(flatten)]
    pub row: vault_sessions::SessionRow,
    pub running: bool,
}

#[derive(serde::Serialize)]
pub struct ListSessionsResponse {
    pub items: Vec<SessionListItem>,
}

pub async fn list_handler(
    State(state): State<AppState>,
) -> Result<Json<ListSessionsResponse>, AppError> {
    let rows = vault_sessions::list(&state.pool, &state.user_id).await?;
    let active = state.active_replies.lock().await;
    let items = rows
        .into_iter()
        .map(|row| {
            let running = active
                .get(&row.id)
                .map(|task| !task.token.is_cancelled())
                .unwrap_or(false);
            SessionListItem { row, running }
        })
        .collect();
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

    // Tool evidence now lives inline in the queue (assistant_tool_calls /
    // tool_result rows), which render_transcript renders directly — no separate
    // supporting-context side channel needed.
    let summary = agent::compact::summarize_session(provider, history, focus, cancel).await?;

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
