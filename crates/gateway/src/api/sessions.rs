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

use super::{AppError, AppState};
use crate::agent;
use crate::events::EventEnvelope;
use crate::vault::{
    compactions as vault_compactions, events as vault_events, messages as vault_messages,
    sessions as vault_sessions,
};

pub async fn abort_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> StatusCode {
    let map = state.active_replies.lock().await;
    if let Some(token) = map.get(&session_id) {
        token.cancel();
        tracing::info!(session_id, "abort signalled");
    }
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
    let rows = vault_events::list_for_session(
        &state.pool,
        &state.user_id,
        &session_id,
        q.since,
        q.limit,
    )
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
    vault_sessions::ensure_exists(
        &state.pool,
        &state.user_id,
        &id,
        body.title.as_deref(),
    )
    .await?;
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
    // Cancel any in-flight reply for this session before yanking the data.
    {
        let mut map = state.active_replies.lock().await;
        if let Some(token) = map.remove(&session_id) {
            token.cancel();
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

#[derive(serde::Serialize)]
pub struct CompactAcceptedResponse {
    /// New session forked from `id`. Frontend should navigate to this once
    /// it receives the `compaction.completed` SSE event.
    pub new_session_id: String,
}

/// `POST /api/v1/sessions/{id}/compact` — fork the session into a new one
/// whose head is a structured handoff summary, freeing context budget. The
/// endpoint returns 202 immediately and runs the summary call in the
/// background; subscribers on the original session's SSE stream see
/// `compaction.started` / `compaction.completed` / `compaction.aborted`.
pub async fn compact_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<CompactBody>,
) -> Result<(StatusCode, Json<CompactAcceptedResponse>), AppError> {
    // Reject if there's a *live* in-flight reply / compaction on this
    // session — compacting mid-stream would race with vault writes and
    // confuse the UI. A cancelled-but-not-yet-cleaned-up token is fine
    // (someone hit /abort and the task is on its way out).
    {
        let map = state.active_replies.lock().await;
        if let Some(t) = map.get(&session_id) {
            if !t.is_cancelled() {
                return Err(AppError(anyhow::anyhow!(
                    "compaction rejected: agent reply in progress; abort or wait first"
                )));
            }
        }
    }

    // Pre-allocate the new session id so we can return it immediately and
    // the frontend can subscribe to it before the background task finishes.
    let new_session_id = format!("s-{}", uuid::Uuid::new_v4().simple());
    let trigger = body
        .trigger
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "manual".to_string());

    // Compaction shares the active_replies map with normal replies — same
    // token, same /abort endpoint cancels both. Keeps the UX rule "one
    // in-flight thing per session, Esc cancels it" consistent.
    let cancel = CancellationToken::new();
    {
        let mut map = state.active_replies.lock().await;
        map.insert(session_id.clone(), cancel.clone());
    }

    // Fire the start event synchronously so the UI flips into "compacting"
    // immediately, before the slow summarizer call.
    agent::publish_and_persist(
        &state.pool,
        &state.user_id,
        &session_id,
        None,
        &state.event_bus,
        "compaction.started",
        serde_json::json!({
            "trigger": trigger,
            "focus": body.focus,
            "new_session_id": new_session_id,
        }),
    )
    .await?;

    // Spawn background work; POST returns 202 right after this.
    let pool = state.pool.clone();
    let user_id = state.user_id.clone();
    let bus = state.event_bus.clone();
    let provider = state.provider.clone();
    let active_replies = state.active_replies.clone();
    let source_session = session_id.clone();
    let new_id = new_session_id.clone();
    let focus = body.focus.clone();
    let trigger_for_task = trigger.clone();
    let cancel_for_task = cancel.clone();
    let cancel_for_cleanup = cancel.clone();

    tokio::spawn(async move {
        let outcome = run_compaction(
            pool.clone(),
            user_id.clone(),
            source_session.clone(),
            new_id.clone(),
            provider,
            focus.as_deref(),
            &trigger_for_task,
            cancel_for_task,
        )
        .await;

        // Drop the cancel token unless we were replaced by a later /compact
        // or POST (which would have cancelled us); the new owner keeps the
        // slot in that case.
        if !cancel_for_cleanup.is_cancelled() {
            active_replies.lock().await.remove(&source_session);
        }

        let (kind, payload) = match outcome {
            Ok(report) => (
                "compaction.completed".to_string(),
                serde_json::json!({
                    "new_session_id": new_id,
                    "summary_md": report.summary_md,
                    "messages_removed": report.messages_removed,
                    "messages_retained": report.messages_retained,
                    "trigger": trigger_for_task,
                }),
            ),
            Err(err) => {
                // Surface the full anyhow chain so failed compactions are
                // diagnosable from server logs (the outermost context alone
                // is rarely informative — e.g. "compact: chat call").
                tracing::warn!(error = ?err, "compaction failed");
                (
                    "compaction.aborted".to_string(),
                    serde_json::json!({
                        "new_session_id": new_id,
                        "reason": format!("{err:#}"),
                    }),
                )
            }
        };

        if let Err(e) = agent::publish_and_persist(
            &pool,
            &user_id,
            &source_session,
            None,
            &bus,
            &kind,
            payload,
        )
        .await
        {
            tracing::error!(error = %e, "failed to publish compaction terminal event");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(CompactAcceptedResponse { new_session_id }),
    ))
}

/// Outcome record for a successful compaction — drives the
/// `compaction.completed` event payload + the session_compactions row.
struct CompactionReport {
    summary_md: String,
    messages_removed: i64,
    messages_retained: i64,
}

#[allow(clippy::too_many_arguments)]
async fn run_compaction(
    pool: sqlx::SqlitePool,
    user_id: String,
    source_session: String,
    new_session: String,
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    focus: Option<&str>,
    trigger: &str,
    cancel: CancellationToken,
) -> anyhow::Result<CompactionReport> {
    // Load full history of source session. The hard-coded LIMIT here matches
    // run_chat_reply — bumped to 1000 in #133 once that lands; for now we
    // accept the same ceiling everyone else uses.
    let history =
        vault_messages::list(&pool, &user_id, &source_session, None, 1000).await?;
    if history.is_empty() {
        anyhow::bail!("source session has no messages to compact");
    }
    let messages_removed = history.len() as i64;

    // Run structured summary (gpt-5.5, reasoning_effort=Minimal).
    let summary =
        agent::compact::summarize_session(provider, &history, focus, cancel).await?;

    // Fork the new session.
    vault_sessions::fork(
        &pool,
        &user_id,
        &source_session,
        &new_session,
        Some("Compacted"),
        trigger,
    )
    .await?;

    // Write the summary as the first message of the new session, role
    // `compaction_summary` so the agent loop can inject it into the system
    // prompt instead of the message list (lands in #136).
    vault_messages::insert(
        &pool,
        &user_id,
        &new_session,
        "compaction_summary",
        &serde_json::json!({ "type": "text", "text": summary }),
        None,
    )
    .await?;

    // Audit row.
    vault_compactions::insert(
        &pool,
        &user_id,
        &source_session,
        &new_session,
        &summary,
        /* messages_retained */ 1, // just the summary in the new session
        messages_removed,
        /* tokens_before */ None, // wired in #136 with llm_usage_log lookup
        /* tokens_after */ None,
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
