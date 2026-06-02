//! `codex_oauth` provider — talks to ChatGPT subscription backend.
//!
//! See `design/p1-spec/llm-provider.md` §4.3 + §5 for protocol details.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::{stream::BoxStream, SinkExt, StreamExt};
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::auth::codex::{self, CodexTokens};
use crate::vault::provider_configs;

use super::openai_responses;
use super::{ChatRequest, LlmEvent, LlmProvider};

const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const WS_URL: &str = "wss://chatgpt.com/backend-api/codex/responses";
const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const REFRESH_SKEW_SECS: i64 = 60;

type CodexWsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub struct CodexOauthProvider {
    pool: SqlitePool,
    user_id: String,
    http: reqwest::Client,
    cached: Arc<Mutex<Option<CodexTokens>>>,
    ws: Arc<Mutex<CodexWsState>>,
}

#[derive(Default)]
struct CodexWsState {
    conversations: HashMap<String, CodexWsConversation>,
}

struct CodexWsConversation {
    stream: CodexWsStream,
    last_full_input: Vec<Value>,
    last_response_id: Option<String>,
    last_items_added: Vec<Value>,
}

impl CodexOauthProvider {
    pub fn new(pool: SqlitePool, user_id: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            // Long-running SSE streams shouldn't time out mid-response.
            // Per-request timeout is conservative; kept-alive idle is short.
            .pool_idle_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()
            .context("building reqwest client for codex_oauth")?;
        Ok(Self {
            pool,
            user_id: user_id.into(),
            http,
            cached: Arc::new(Mutex::new(None)),
            ws: Arc::new(Mutex::new(CodexWsState::default())),
        })
    }

    /// Return current access_token, refreshing if it's near expiry.
    async fn ensure_fresh_token(&self) -> Result<String> {
        let mut cached = self.cached.lock().await;

        if cached.is_none() {
            let row = provider_configs::get_codex(&self.pool, &self.user_id)
                .await?
                .ok_or_else(|| {
                    anyhow!(
                        "codex_oauth not configured for user '{}'. Run `leek auth codex` first.",
                        self.user_id
                    )
                })?;
            *cached = Some(CodexTokens {
                access_token: row.access_token,
                refresh_token: row.refresh_token,
                expires_at: row.expires_at,
            });
        }

        if needs_refresh(cached.as_ref().unwrap().expires_at) {
            let current = cached.as_ref().unwrap().clone();
            tracing::info!(
                expires_at = %current.expires_at,
                "refreshing codex access_token"
            );
            let new = codex::refresh(&self.http, &current.refresh_token)
                .await
                .context("refreshing codex token")?;
            provider_configs::upsert_codex(&self.pool, &self.user_id, &new).await?;
            *cached = Some(new);
        }

        Ok(cached.as_ref().unwrap().access_token.clone())
    }
}

fn needs_refresh(expires_at: DateTime<Utc>) -> bool {
    let skew = chrono::Duration::seconds(REFRESH_SKEW_SECS);
    expires_at - Utc::now() < skew
}

#[async_trait]
impl LlmProvider for CodexOauthProvider {
    fn name(&self) -> &str {
        "codex_oauth"
    }

    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
        let access_token = self.ensure_fresh_token().await?;
        let identity_headers = codex_identity_headers(req.session_id.as_deref());
        let mut body = openai_responses::build_request_body(&req);
        if let Some(obj) = body.as_object_mut() {
            obj.remove("prompt_cache_retention");
        }
        if use_websocket_transport() {
            return Ok(stream_websocket(
                Arc::clone(&self.ws),
                access_token,
                identity_headers,
                ws_conversation_key(&req),
                body,
            ));
        }
        let url = format!("{BASE_URL}/responses");

        let mut resp =
            post_responses(&self.http, &url, &access_token, &body, &identity_headers).await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            if prompt_cache_retention_is_unsupported(status, &body_text)
                && body.get("prompt_cache_retention").is_some()
            {
                tracing::warn!(
                    "codex backend rejected prompt_cache_retention; retrying without retention"
                );
                if let Some(obj) = body.as_object_mut() {
                    obj.remove("prompt_cache_retention");
                }
                resp = post_responses(&self.http, &url, &access_token, &body, &identity_headers)
                    .await?;
                let status = resp.status();
                if status.is_success() {
                    return Ok(openai_responses::parse_sse_stream(resp));
                }
                let body_text = resp.text().await.unwrap_or_default();
                bail!("codex /responses returned {status}: {body_text}");
            }
            bail!("codex /responses returned {status}: {body_text}");
        }

        Ok(openai_responses::parse_sse_stream(resp))
    }
}

fn use_websocket_transport() -> bool {
    std::env::var("LEEK_CODEX_USE_WEBSOCKET")
        .map(|v| v != "0")
        .unwrap_or(false)
}

fn ws_conversation_key(req: &ChatRequest) -> String {
    format!(
        "{}:{}:{}",
        req.session_id.as_deref().unwrap_or("adhoc"),
        req.model,
        req.prompt_cache_key.as_deref().unwrap_or("default")
    )
}

fn stream_websocket(
    ws_state: Arc<Mutex<CodexWsState>>,
    access_token: String,
    identity_headers: Vec<(&'static str, String)>,
    conversation_key: String,
    body: Value,
) -> BoxStream<'static, Result<LlmEvent>> {
    Box::pin(try_stream! {
        let mut state = ws_state.lock().await;
        let mut conversation = match state.conversations.remove(&conversation_key) {
            Some(conversation) => conversation,
            None => new_ws_conversation(&access_token, &identity_headers, &body).await?,
        };

        let current_input = response_input(&body);
        let mut request_body = websocket_create_body(body);
        if let Some((previous_response_id, delta_input)) =
            incremental_ws_input(&conversation, &current_input)
        {
            request_body["previous_response_id"] = Value::String(previous_response_id);
            request_body["input"] = Value::Array(delta_input);
        }

        let mut response_id = None;
        let mut items_added = Vec::new();
        send_ws_json(&mut conversation.stream, &request_body).await?;

        loop {
            let message = match conversation.stream.next().await {
                Some(message) => message,
                None => Err(anyhow!("codex websocket closed before response.completed"))?,
            };
            let message = message.context("reading codex websocket message")?;
            let text = match message {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => String::from_utf8(bytes.to_vec())
                    .context("non-UTF-8 codex websocket binary message")?,
                Message::Close(frame) => {
                    Err(anyhow!(
                        "codex websocket closed before response.completed: {frame:?}"
                    ))?
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            };

            let completed = observe_ws_event(&text, &mut response_id, &mut items_added)?;
            for event in openai_responses::parse_json_event(&text)? {
                yield event;
            }
            if completed {
                break;
            }
        }

        conversation.last_full_input = current_input;
        conversation.last_response_id = response_id;
        conversation.last_items_added = items_added;
        state.conversations.insert(conversation_key, conversation);
    })
}

async fn new_ws_conversation(
    access_token: &str,
    identity_headers: &[(&'static str, String)],
    body: &Value,
) -> Result<CodexWsConversation> {
    let mut request = WS_URL
        .into_client_request()
        .context("building codex websocket request")?;
    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {access_token}"))
            .context("building websocket Authorization header")?,
    );
    headers.insert(
        "OpenAI-Beta",
        HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA),
    );
    for (name, value) in identity_headers {
        headers.insert(
            *name,
            HeaderValue::from_str(value)
                .with_context(|| format!("building websocket {name} header"))?,
        );
    }

    let (mut stream, _) = connect_async(request)
        .await
        .context("connecting codex websocket")?;
    let mut warmup = websocket_create_body(body.clone());
    warmup["input"] = Value::Array(Vec::new());
    warmup["generate"] = Value::Bool(false);
    send_ws_json(&mut stream, &warmup).await?;

    let mut response_id = None;
    loop {
        let Some(message) = stream.next().await else {
            bail!("codex websocket closed during warmup");
        };
        let message = message.context("reading codex websocket warmup")?;
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8(bytes.to_vec())
                .context("non-UTF-8 codex websocket warmup binary message")?,
            Message::Close(frame) => bail!("codex websocket closed during warmup: {frame:?}"),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        let mut ignored_items = Vec::new();
        if observe_ws_event(&text, &mut response_id, &mut ignored_items)? {
            break;
        }
    }

    Ok(CodexWsConversation {
        stream,
        last_full_input: Vec::new(),
        last_response_id: response_id,
        last_items_added: Vec::new(),
    })
}

fn websocket_create_body(mut body: Value) -> Value {
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "type".to_string(),
            Value::String("response.create".to_string()),
        );
    }
    body
}

async fn send_ws_json(stream: &mut CodexWsStream, body: &Value) -> Result<()> {
    let text = serde_json::to_string(body).context("serializing codex websocket request")?;
    stream
        .send(Message::Text(text.into()))
        .await
        .context("sending codex websocket request")
}

fn response_input(body: &Value) -> Vec<Value> {
    body.get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn incremental_ws_input(
    conversation: &CodexWsConversation,
    current_input: &[Value],
) -> Option<(String, Vec<Value>)> {
    incremental_ws_input_from_parts(
        &conversation.last_full_input,
        &conversation.last_items_added,
        conversation.last_response_id.as_deref(),
        current_input,
    )
}

fn incremental_ws_input_from_parts(
    last_full_input: &[Value],
    last_items_added: &[Value],
    last_response_id: Option<&str>,
    current_input: &[Value],
) -> Option<(String, Vec<Value>)> {
    let previous_response_id = last_response_id?.to_string();
    let mut baseline = last_full_input.to_vec();
    baseline.extend(last_items_added.to_vec());
    starts_with_items(current_input, &baseline).then(|| {
        (
            previous_response_id,
            current_input[baseline.len()..].to_vec(),
        )
    })
}

fn starts_with_items(items: &[Value], prefix: &[Value]) -> bool {
    items.len() >= prefix.len() && items.iter().zip(prefix).all(|(a, b)| a == b)
}

fn observe_ws_event(
    text: &str,
    response_id: &mut Option<String>,
    items_added: &mut Vec<Value>,
) -> Result<bool> {
    let value: Value = serde_json::from_str(text).context("parsing codex websocket event")?;
    let event_type = value.get("type").and_then(Value::as_str);
    if matches!(
        event_type,
        Some("response.created" | "response.in_progress" | "response.completed")
    ) {
        if let Some(id) = value
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
        {
            *response_id = Some(id.to_string());
        }
    }
    if event_type == Some("response.output_item.done") {
        if let Some(item) = value.get("item").and_then(replay_baseline_item) {
            items_added.push(item);
        }
    }
    Ok(event_type == Some("response.completed"))
}

fn replay_baseline_item(item: &Value) -> Option<Value> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    match item_type {
        "function_call" => Some(serde_json::json!({
            "type": "function_call",
            "call_id": item.get("call_id")?.clone(),
            "name": item.get("name")?.clone(),
            "arguments": item.get("arguments").cloned().unwrap_or(Value::String(String::new())),
        })),
        "reasoning" if use_reasoning_replay() => {
            let encrypted_content = item.get("encrypted_content")?.clone();
            Some(serde_json::json!({
                "type": "reasoning",
                "summary": item.get("summary").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
                "encrypted_content": encrypted_content,
            }))
        }
        _ => None,
    }
}

fn use_reasoning_replay() -> bool {
    std::env::var("LEEK_REPLAY_REASONING")
        .map(|v| v != "0")
        .unwrap_or(false)
}

async fn post_responses(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    body: &serde_json::Value,
    identity_headers: &[(&'static str, String)],
) -> Result<reqwest::Response> {
    let mut request = http
        .post(url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream");

    for (name, value) in identity_headers {
        request = request.header(*name, value);
    }

    request
        .json(body)
        .send()
        .await
        .context("POST /responses to codex backend")
}

fn codex_identity_headers(session_id: Option<&str>) -> Vec<(&'static str, String)> {
    let Some(session_id) = session_id.and_then(codex_header_value) else {
        return Vec::new();
    };
    let thread_id = session_id.clone();
    vec![
        ("session-id", session_id),
        ("thread-id", thread_id.clone()),
        ("x-client-request-id", thread_id.clone()),
        ("x-codex-window-id", format!("{thread_id}:0")),
    ]
}

fn codex_header_value(raw: &str) -> Option<String> {
    let value = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':') {
                c
            } else {
                '-'
            }
        })
        .take(120)
        .collect::<String>();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn prompt_cache_retention_is_unsupported(status: reqwest::StatusCode, body_text: &str) -> bool {
    status == reqwest::StatusCode::BAD_REQUEST
        && body_text.contains("prompt_cache_retention")
        && (body_text.contains("Unsupported")
            || body_text.contains("unsupported")
            || body_text.contains("unknown")
            || body_text.contains("unrecognized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_cache_retention_error_is_retryable_only_for_400_unsupported() {
        assert!(prompt_cache_retention_is_unsupported(
            reqwest::StatusCode::BAD_REQUEST,
            "Unsupported parameter: prompt_cache_retention"
        ));
        assert!(!prompt_cache_retention_is_unsupported(
            reqwest::StatusCode::BAD_REQUEST,
            "invalid model"
        ));
        assert!(!prompt_cache_retention_is_unsupported(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "Unsupported parameter: prompt_cache_retention"
        ));
    }

    #[test]
    fn codex_identity_headers_use_stable_session_id() {
        let headers = codex_identity_headers(Some("s-abc/含中文"));

        assert_eq!(
            headers,
            vec![
                ("session-id", "s-abc----".to_string()),
                ("thread-id", "s-abc----".to_string()),
                ("x-client-request-id", "s-abc----".to_string()),
                ("x-codex-window-id", "s-abc----:0".to_string()),
            ]
        );
    }

    #[test]
    fn codex_identity_headers_are_absent_without_session() {
        assert!(codex_identity_headers(None).is_empty());
    }

    #[test]
    fn incremental_ws_input_sends_only_new_tail_after_function_call() {
        let user = serde_json::json!({"role": "user", "content": "do it"});
        let call = serde_json::json!({
            "type": "function_call",
            "call_id": "c1",
            "name": "lookup",
            "arguments": "{}"
        });
        let output = serde_json::json!({
            "type": "function_call_output",
            "call_id": "c1",
            "output": "ok"
        });

        let result = incremental_ws_input_from_parts(
            std::slice::from_ref(&user),
            std::slice::from_ref(&call),
            Some("resp_1"),
            &[user.clone(), call.clone(), output.clone()],
        )
        .unwrap();

        assert_eq!(result.0, "resp_1");
        assert_eq!(result.1, vec![output]);
    }

    #[test]
    fn incremental_ws_input_rejects_non_prefix_history() {
        let result = incremental_ws_input_from_parts(
            &[serde_json::json!({"role": "user", "content": "old"})],
            &[],
            Some("resp_1"),
            &[serde_json::json!({"role": "user", "content": "new"})],
        );

        assert!(result.is_none());
    }

    #[test]
    fn replay_baseline_item_normalizes_function_call_for_leek_replay() {
        let item = serde_json::json!({
            "id": "fc_1",
            "type": "function_call",
            "call_id": "c1",
            "name": "lookup",
            "arguments": "{\"x\":1}"
        });

        assert_eq!(
            replay_baseline_item(&item).unwrap(),
            serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "lookup",
                "arguments": "{\"x\":1}"
            })
        );
    }
}
