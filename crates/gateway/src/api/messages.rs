use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{AppError, AppState};
use crate::agent;
use crate::events::EventEnvelope;
use crate::vault::{events as vault_events, messages as vault_messages, sessions as vault_sessions};

#[derive(Deserialize)]
pub struct PostMessageBody {
    /// Optional task thread to attach this message to. Routing layer will
    /// honor it once #48 grows the full extraction logic; ignored for the
    /// chat_reply slice.
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
pub struct PostMessageResponse {
    pub message_seq: i64,
}

pub async fn post_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<PostMessageBody>,
) -> Result<(StatusCode, Json<PostMessageResponse>), AppError> {
    // Lazy-create session row on first message
    vault_sessions::ensure_exists(&state.pool, &state.user_id, &session_id, None).await?;

    let user_text = match &body.content {
        ContentPart::Text { text } => text.clone(),
    };

    let user_seq = vault_messages::insert(
        &state.pool,
        &state.user_id,
        &session_id,
        "user",
        &serde_json::json!({ "type": "text", "text": user_text }),
    )
    .await?;

    // Emit user_message event so SSE subscribers see the user's input echoed
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

    // Fire-and-forget agent reply — events stream out via EventBus.
    // The agent reads full history from vault, so multi-turn context flows naturally.
    let _ = user_text; // kept to make intent explicit; agent reads from vault.
    let pool = state.pool.clone();
    let user_id = state.user_id.clone();
    let sess = session_id.clone();
    let provider = state.provider.clone();
    let bus = state.event_bus.clone();
    tokio::spawn(async move {
        if let Err(e) = agent::run_chat_reply(pool, user_id, sess, provider, bus).await {
            tracing::error!(error = %e, "agent reply failed");
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(PostMessageResponse {
            message_seq: user_seq,
        }),
    ))
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
