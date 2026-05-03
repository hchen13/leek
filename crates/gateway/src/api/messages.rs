use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{AppError, AppState};
use crate::agent::routing::{self, DecisionKind};
use crate::agent::{self, TaskBinding};
use crate::events::EventEnvelope;
use crate::llm::{ChatMessage, Role};
use crate::vault::{
    events as vault_events, messages as vault_messages, sessions as vault_sessions,
    tasks as vault_tasks,
};

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
        None, // task_id will be back-filled if routing decides new_task
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
        if let Some(prev) = map.insert(session_id.clone(), cancel.clone()) {
            prev.cancel();
        }
    }

    // Route + reply in the background — POST returns 201 immediately.
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
        // Clear the cancel token after the reply finishes so /compact knows
        // the session is idle. Skip if we were replaced by a later POST —
        // replacement cancels the previous token; if ours is cancelled, the
        // map slot belongs to someone else now.
        if !cancel_for_cleanup.is_cancelled() {
            active_replies.lock().await.remove(&sess);
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(PostMessageResponse {
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
    // P1 simplification: routing layer fires only when there's no in-progress
    // task. With one, attach to it directly (in-thread chat_reply).
    let active = vault_tasks::get_active_for_session(&pool, &user_id, &session_id).await?;
    if let Some(task) = active {
        // Link the user message to the existing task and reply within it.
        vault_tasks::link_message(&pool, &user_id, &session_id, user_message_seq, &task.id).await?;
        return agent::run_chat_reply(
            pool,
            user_id,
            session_id,
            provider,
            event_bus,
            // Existing task: don't re-mark as delivered (it already is or in-flight)
            None,
            cancel,
            tools,
        )
        .await;
    }

    // No active task — run the routing layer.
    let history = load_history_for_routing(&pool, &user_id, &session_id).await?;
    let decision = match routing::decide_route(provider.clone(), &history, &user_text).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "routing failed; falling back to chat_reply");
            // Fallback: just run the main reply pipeline
            return agent::run_chat_reply(
                pool,
                user_id,
                session_id,
                provider,
                event_bus,
                None,
                cancel,
                tools,
            )
            .await;
        }
    };

    match decision.kind {
        DecisionKind::NewTask => {
            let draft = decision.task_draft.ok_or_else(|| {
                anyhow::anyhow!("routing returned new_task without task_draft")
            })?;
            let task_id = vault_tasks::insert_in_progress(
                &pool,
                &user_id,
                &session_id,
                vault_tasks::NewTask {
                    title: &draft.title,
                    goal: &draft.goal,
                    expected_deliverable: &draft.expected_deliverable,
                    source: "user",
                    constraints_json: None,
                    context_refs_json: None,
                },
            )
            .await?;

            vault_tasks::link_message(&pool, &user_id, &session_id, user_message_seq, &task_id)
                .await?;

            agent::publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                Some(&task_id),
                &event_bus,
                "task_created",
                serde_json::json!({
                    "task_id": task_id,
                    "title": draft.title,
                    "goal": draft.goal,
                    "expected_deliverable": draft.expected_deliverable,
                    "reason": decision.reason,
                }),
            )
            .await?;
            agent::publish_and_persist(
                &pool,
                &user_id,
                &session_id,
                Some(&task_id),
                &event_bus,
                "task_started",
                serde_json::json!({
                    "task_id": task_id,
                    "title": draft.title,
                }),
            )
            .await?;

            agent::run_chat_reply(
                pool,
                user_id,
                session_id,
                provider,
                event_bus,
                Some(TaskBinding {
                    task_id,
                    expected_deliverable: draft.expected_deliverable,
                }),
                cancel,
                tools,
            )
            .await
        }

        DecisionKind::ChatReply => {
            agent::run_chat_reply(
                pool, user_id, session_id, provider, event_bus, None, cancel, tools,
            )
            .await
        }

        DecisionKind::Ambiguous => {
            // Routing layer already produced the clarification text; surface it
            // as an agent message + clarification_requested event without burning
            // a second LLM call.
            let question = decision.clarification_question.unwrap_or_else(|| {
                "Could you clarify what you'd like me to look into?".to_string()
            });
            simulate_agent_reply(
                &pool,
                &user_id,
                &session_id,
                &event_bus,
                &question,
                Some("clarification_requested"),
            )
            .await
        }
    }
}

async fn load_history_for_routing(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    session_id: &str,
) -> anyhow::Result<Vec<ChatMessage>> {
    let rows = vault_messages::list(pool, user_id, session_id, None, 100).await?;
    let messages: Vec<ChatMessage> = rows
        .iter()
        .filter_map(|row| {
            let content: serde_json::Value = serde_json::from_str(&row.content_json).ok()?;
            let text = content.get("text")?.as_str()?.to_string();
            let role = match row.role.as_str() {
                "user" => Role::User,
                "agent" => Role::Assistant,
                _ => return None,
            };
            Some(ChatMessage { role, content: text })
        })
        .collect();
    Ok(messages)
}

/// Stream an agent message that doesn't come from an LLM (clarifications etc).
/// Mirrors the event sequence the live UI expects so it renders identically
/// to a real LLM stream.
async fn simulate_agent_reply(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    session_id: &str,
    event_bus: &crate::events::EventBus,
    text: &str,
    extra_kind_before: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(kind) = extra_kind_before {
        agent::publish_and_persist(
            pool,
            user_id,
            session_id,
            None,
            event_bus,
            kind,
            serde_json::json!({ "question": text }),
        )
        .await?;
    }

    agent::publish_and_persist(
        pool,
        user_id,
        session_id,
        None,
        event_bus,
        "agent_message_start",
        serde_json::json!({}),
    )
    .await?;

    agent::publish_and_persist(
        pool,
        user_id,
        session_id,
        None,
        event_bus,
        "agent_message_delta",
        serde_json::json!({ "text": text }),
    )
    .await?;

    let msg_seq = vault_messages::insert(
        pool,
        user_id,
        session_id,
        "agent",
        &serde_json::json!({ "type": "text", "text": text }),
        None,
    )
    .await?;

    agent::publish_and_persist(
        pool,
        user_id,
        session_id,
        None,
        event_bus,
        "agent_message_end",
        serde_json::json!({
            "stop_reason": "end_turn",
            "message_seq": msg_seq,
        }),
    )
    .await?;

    Ok(())
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
