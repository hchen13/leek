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
    let trigger = body
        .trigger
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "manual".to_string());
    let new_session_id = start_compaction(&state, &session_id, &trigger, body.focus.as_deref())
        .await
        .map_err(AppError)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CompactAcceptedResponse { new_session_id }),
    ))
}

/// Kick off a compaction on `session_id`. Common path used by both manual
/// `POST /compact` and the auto-pre-turn trigger inside `messages::post_handler`.
/// Returns the pre-allocated new session id; the actual summary work runs
/// in a background task and emits SSE `compaction.{started,completed,aborted}`
/// events on the source session.
///
/// The cancel token goes into `active_replies` regardless of trigger; the
/// frontend decides whether `/abort` is offered (manual: yes; auto: no), but
/// the backend doesn't enforce that distinction — auto compactions are simply
/// not surfaced in UI as cancellable.
pub async fn start_compaction(
    state: &AppState,
    session_id: &str,
    trigger: &str,
    focus: Option<&str>,
) -> anyhow::Result<String> {
    // Reject if there's a *live* in-flight reply / compaction on this
    // session — racing with vault writes would confuse the UI. A cancelled-
    // but-not-yet-cleaned-up token is fine (someone hit /abort already).
    {
        let map = state.active_replies.lock().await;
        if let Some(task) = map.get(session_id) {
            if !task.token.is_cancelled() {
                anyhow::bail!(
                    "compaction rejected: agent reply in progress; abort or wait first"
                );
            }
        }
    }

    let new_session_id = format!("s-{}", uuid::Uuid::new_v4().simple());

    let cancel = CancellationToken::new();
    // Auto pre-turn compactions are mandatory (see ActiveTask docs); manual
    // ones can be aborted with Esc.
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

    // Fire the start event synchronously so any subscriber flips into
    // "compacting" before the slow summarizer call.
    agent::publish_and_persist(
        &state.pool,
        &state.user_id,
        session_id,
        None,
        &state.event_bus,
        "compaction.started",
        serde_json::json!({
            "trigger": trigger,
            "focus": focus,
            "new_session_id": new_session_id,
        }),
    )
    .await?;

    let pool = state.pool.clone();
    let user_id = state.user_id.clone();
    let bus = state.event_bus.clone();
    let provider = state.provider.clone();
    let active_replies = state.active_replies.clone();
    let source_session = session_id.to_string();
    let new_id = new_session_id.clone();
    let focus_owned = focus.map(|s| s.to_string());
    let trigger_for_task = trigger.to_string();
    let cancel_for_task = cancel.clone();
    let cancel_for_cleanup = cancel.clone();

    tokio::spawn(async move {
        let outcome = run_compaction(
            pool.clone(),
            user_id.clone(),
            source_session.clone(),
            new_id.clone(),
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
                    "new_session_id": new_id,
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
                        "new_session_id": new_id,
                        "reason": format!("{err:#}"),
                        "trigger": trigger_for_task,
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

    Ok(new_session_id)
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
