use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::sessions as api_sessions;
use super::{AppError, AppState};
use crate::agent;
use crate::events::EventEnvelope;
use crate::vault::{
    events as vault_events, messages as vault_messages, sessions as vault_sessions,
};

/// Auto-compaction threshold. When the session's last `llm_usage` event
/// reports `input_tokens` ≥ this value, the next user message triggers a
/// in-place compaction *before* the LLM is called again — otherwise the next
/// turn would push the request over the model's context window.
///
/// Trigger at 95% of the 400K context window (= 380K), leaving ~20K for the
/// working turn's user message, system prompt delta, and tool outputs.
/// Override via `LEEK_AUTO_COMPACT_THRESHOLD` for tests / low-budget tiers.
const AUTO_COMPACT_THRESHOLD_DEFAULT: i64 = 380_000;

fn auto_compact_threshold() -> i64 {
    std::env::var("LEEK_AUTO_COMPACT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(AUTO_COMPACT_THRESHOLD_DEFAULT)
}

#[derive(Deserialize)]
pub struct PostMessageBody {
    /// Optional task thread to attach this message to once task binding is
    /// wired through the message endpoint; ignored for the chat_reply slice.
    #[serde(default)]
    #[allow(dead_code)]
    pub task_id: Option<String>,
    pub content: ContentPart,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum PostMessageResponse {
    /// User message accepted, agent reply is starting.
    Created { message_seq: i64 },
    /// Token budget exceeded — in-place compaction kicked off; the user
    /// message was *not* persisted and should be re-sent to the same session
    /// once `compaction.completed` arrives on the SSE stream.
    AutoCompacting { auto_compacting: bool },
}

pub async fn post_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PostMessageBody>,
) -> Result<(StatusCode, Json<PostMessageResponse>), AppError> {
    vault_sessions::ensure_exists(&state.pool, &state.user_id, &session_id, None).await?;

    let user_text = match &body.content {
        ContentPart::Text { text } => text.clone(),
    };

    // Pre-turn auto-compaction: if the most recent LLM call on this session
    // already saw input_tokens ≥ threshold, reject this user message and
    // start compaction. Frontend queues the message and re-POSTs it to this
    // same session after `compaction.completed`.
    let threshold = auto_compact_threshold();
    if let Some(latest) =
        vault_events::latest_input_tokens(&state.pool, &state.user_id, &session_id).await?
    {
        if latest >= threshold {
            tracing::info!(
                session_id,
                latest_input_tokens = latest,
                threshold,
                "auto-compact triggered"
            );
            api_sessions::start_compaction(&state, &session_id, "auto_pre_turn", None)
                .await
                .map_err(AppError)?;
            return Ok((
                StatusCode::ACCEPTED,
                Json(PostMessageResponse::AutoCompacting {
                    auto_compacting: true,
                }),
            ));
        }
    }

    let user_seq = vault_messages::insert(
        &state.pool,
        &state.user_id,
        &session_id,
        "user",
        &serde_json::json!({ "type": "text", "text": user_text }),
        None,
    )
    .await?;

    // Echo user_message event for SSE subscribers
    let payload = serde_json::json!({ "text": user_text, "seq": user_seq });
    let ts = chrono::Utc::now();
    let evt_seq = vault_events::insert(
        &state.pool,
        &state.user_id,
        &session_id,
        None,
        "user_message",
        &payload,
        Some("user"),
        ts,
    )
    .await?;
    state
        .event_bus
        .publish(
            &session_id,
            EventEnvelope::new(evt_seq, "user_message", payload),
        )
        .await;

    // Each session has at most one in-flight reply. New POST cancels the previous.
    let cancel = CancellationToken::new();
    {
        let mut map = state.active_replies.lock().await;
        if let Some(prev) = map.insert(
            session_id.clone(),
            super::ActiveTask {
                token: cancel.clone(),
                user_cancellable: true,
            },
        ) {
            prev.token.cancel();
        }
    }

    // Reply in the background — POST returns 201 immediately.
    let pool = state.pool.clone();
    let user_id = state.user_id.clone();
    let sess = session_id.clone();
    let provider = state.provider.clone();
    let bus = state.event_bus.clone();
    let tools = state.tools.clone();
    let cancel_for_task = cancel.clone();
    let cancel_for_cleanup = cancel.clone();
    let active_replies = state.active_replies.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_user_message(
            pool,
            user_id,
            sess.clone(),
            provider,
            bus,
            user_text,
            user_seq,
            cancel_for_task,
            tools,
        )
        .await
        {
            tracing::error!(error = %e, "agent dispatch failed");
        }
        // Clear the in-flight slot after normal completion or a user abort.
        // If a later POST replaced us, its token is still live and stays put.
        let mut replies = active_replies.lock().await;
        if !cancel_for_cleanup.is_cancelled()
            || replies
                .get(&sess)
                .map(|task| task.token.is_cancelled())
                .unwrap_or(false)
        {
            replies.remove(&sess);
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(PostMessageResponse::Created {
            message_seq: user_seq,
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn handle_user_message(
    pool: sqlx::SqlitePool,
    user_id: String,
    session_id: String,
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    event_bus: crate::events::EventBus,
    user_text: String,
    user_message_seq: i64,
    cancel: CancellationToken,
    tools: crate::agent::tools::ToolRegistry,
) -> anyhow::Result<()> {
    let _ = (user_text, user_message_seq);
    let result = agent::run_chat_reply(
        pool.clone(),
        user_id.clone(),
        session_id.clone(),
        provider,
        event_bus.clone(),
        cancel.clone(),
        tools,
    )
    .await;

    if let Err(e) = &result {
        if !latest_agent_event_is_terminal(&pool, &user_id, &session_id)
            .await
            .unwrap_or(false)
        {
            let stop_reason = if cancel.is_cancelled() {
                "user_aborted"
            } else {
                "agent_error"
            };
            agent::publish_terminal_failure(
                &pool,
                &user_id,
                &session_id,
                None,
                &event_bus,
                "",
                stop_reason,
                &e.to_string(),
            )
            .await;
        }
    }

    result
}

async fn latest_agent_event_is_terminal(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    session_id: &str,
) -> anyhow::Result<bool> {
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT kind FROM events WHERE user_id = ? AND session_id = ? ORDER BY seq DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(kind
        .map(|kind| kind == "agent_message_end" || kind == "agent_message_failed")
        .unwrap_or(false))
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub since_seq: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn list_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows = vault_messages::list(
        &state.pool,
        &state.user_id,
        &session_id,
        q.since_seq,
        q.limit.unwrap_or(100),
    )
    .await?;
    Ok(Json(serde_json::json!({ "items": rows })))
}
