//! OpenAI Responses API request body + SSE stream parser.
//!
//! Used by `codex_oauth` (talking to chatgpt.com/backend-api/codex/responses)
//! and will be reused by `openai_api_key` (api.openai.com/v1/responses) later.

use anyhow::{anyhow, Context, Result};
use async_stream::try_stream;
use futures::{stream::BoxStream, StreamExt};
use serde::Deserialize;

use super::{ChatRequest, LlmEvent, Role, StopReason, Usage};

pub fn build_request_body(req: &ChatRequest) -> serde_json::Value {
    let mut input = Vec::new();
    for msg in &req.messages {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            // Defensive: callers should put system content in req.system,
            // but if it lands here we still serialize it.
            Role::System => "system",
        };
        input.push(serde_json::json!({
            "role": role,
            "content": msg.content,
        }));
    }
    // The codex backend requires `instructions` (system prompt). OpenAI
    // Responses API treats it as a separate top-level field, not a message.
    let instructions = req
        .system
        .clone()
        .unwrap_or_else(|| "You are a helpful assistant.".to_string());

    let body = serde_json::json!({
        "model": req.model,
        "stream": true,
        "instructions": instructions,
        "input": input,
        // Codex backend disallows server-side response storage; we keep our own
        // history in vault.messages, so this is what we want anyway.
        "store": false,
    });
    // Note: `max_output_tokens` is intentionally NOT serialized — the codex
    // backend rejects it as "Unsupported parameter". We'll add it back when
    // the openai_api_key provider lands (platform.openai.com supports it).
    let _ = req.max_output_tokens;
    body
}

/// Parse an OpenAI Responses SSE stream into our normalized `LlmEvent` flow.
///
/// Events we emit:
/// - `LlmEvent::TextDelta` from `response.output_text.delta`
/// - `LlmEvent::Usage` + `LlmEvent::MessageEnd` from `response.completed`
///
/// Other events (item lifecycle, content_part, etc) are recognized but skipped
/// in P1 — we'll surface them when we need tool calls / reasoning visibility.
pub fn parse_sse_stream(resp: reqwest::Response) -> BoxStream<'static, Result<LlmEvent>> {
    let mut bytes_stream = resp.bytes_stream();

    Box::pin(try_stream! {
        let mut buf = String::new();
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.context("reading SSE chunk")?;
            let s = std::str::from_utf8(&chunk).context("non-UTF-8 in SSE chunk")?;
            buf.push_str(s);

            // Split on blank-line event delimiter; SSE allows \n\n or \r\n\r\n
            loop {
                let Some(idx) = find_event_boundary(&buf) else { break };
                let raw_event = buf[..idx.end_of_event].to_string();
                buf.drain(..idx.next_event_start);
                for evt in parse_one_event(&raw_event)? {
                    yield evt;
                }
            }
        }

        // Flush remaining buffer if it has a complete event without trailing blank line
        if !buf.trim().is_empty() {
            for evt in parse_one_event(&buf)? {
                yield evt;
            }
        }
    })
}

struct EventBoundary {
    end_of_event: usize,
    next_event_start: usize,
}

fn find_event_boundary(buf: &str) -> Option<EventBoundary> {
    if let Some(i) = buf.find("\n\n") {
        return Some(EventBoundary {
            end_of_event: i,
            next_event_start: i + 2,
        });
    }
    if let Some(i) = buf.find("\r\n\r\n") {
        return Some(EventBoundary {
            end_of_event: i,
            next_event_start: i + 4,
        });
    }
    None
}

#[derive(Deserialize)]
struct DataEnvelope<'a> {
    #[serde(rename = "type")]
    type_: Option<&'a str>,
    #[serde(default, borrow)]
    delta: Option<&'a str>,
    #[serde(default)]
    response: Option<ResponseObject>,
}

#[derive(Deserialize)]
struct ResponseObject {
    #[serde(default)]
    usage: Option<UsageRaw>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct UsageRaw {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    input_tokens_details: Option<TokenDetailsRaw>,
}

#[derive(Deserialize)]
struct TokenDetailsRaw {
    #[serde(default)]
    cached_tokens: u32,
}

fn parse_one_event(raw: &str) -> Result<Vec<LlmEvent>> {
    // Concatenate all `data:` lines per SSE spec; ignore `event:`/`id:` lines for now
    // (the `type` field inside the data JSON is what we dispatch on).
    let mut data = String::new();
    for line in raw.lines() {
        let line = line.trim_end();
        if let Some(payload) = line.strip_prefix("data:") {
            let payload = payload.strip_prefix(' ').unwrap_or(payload);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload);
        }
    }
    if data.is_empty() || data == "[DONE]" {
        return Ok(Vec::new());
    }

    let env: DataEnvelope = serde_json::from_str(&data).with_context(|| {
        format!("parsing SSE data line as JSON: {}", truncate(&data, 200))
    })?;

    match env.type_ {
        Some("response.output_text.delta") => {
            let text = env
                .delta
                .ok_or_else(|| anyhow!("output_text.delta event missing 'delta'"))?;
            Ok(vec![LlmEvent::TextDelta {
                text: text.to_string(),
            }])
        }
        Some("response.completed") => {
            let response = env
                .response
                .ok_or_else(|| anyhow!("response.completed missing 'response' object"))?;
            let usage = response.usage.unwrap_or(UsageRaw {
                input_tokens: 0,
                output_tokens: 0,
                input_tokens_details: None,
            });
            let cache_read = usage
                .input_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or(0);
            let stop = match response.status.as_deref() {
                Some("incomplete") => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            };
            Ok(vec![
                LlmEvent::Usage(Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: cache_read,
                    cache_write_tokens: 0,
                }),
                LlmEvent::MessageEnd { stop_reason: stop },
            ])
        }
        Some("response.failed") | Some("response.error") | Some("error") => {
            Err(anyhow!("provider returned error event: {}", truncate(&data, 500)))
        }
        // Lifecycle events we recognize but don't emit anything for (P1 scope)
        Some(_) | None => Ok(Vec::new()),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_delta() {
        let raw = "event: response.output_text.delta\n\
                   data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}";
        let events = parse_one_event(raw).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::TextDelta { text } => assert_eq!(text, "Hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parses_completed_with_usage() {
        let raw = "event: response.completed\n\
                   data: {\"type\":\"response.completed\",\"response\":{\
                       \"status\":\"completed\",\
                       \"usage\":{\"input_tokens\":10,\"output_tokens\":5,\
                                  \"input_tokens_details\":{\"cached_tokens\":3}}}}";
        let events = parse_one_event(raw).unwrap();
        assert_eq!(events.len(), 2);
        match &events[0] {
            LlmEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 5);
                assert_eq!(u.cache_read_tokens, 3);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(
            events[1],
            LlmEvent::MessageEnd {
                stop_reason: StopReason::EndTurn
            }
        ));
    }

    #[test]
    fn ignores_unknown_events() {
        let raw = "event: response.output_item.added\n\
                   data: {\"type\":\"response.output_item.added\",\"item\":{}}";
        assert!(parse_one_event(raw).unwrap().is_empty());
    }

    #[test]
    fn ignores_done_marker() {
        assert!(parse_one_event("data: [DONE]").unwrap().is_empty());
    }

    #[test]
    fn errors_on_failed_event() {
        let raw = "event: response.failed\ndata: {\"type\":\"response.failed\",\"error\":{}}";
        assert!(parse_one_event(raw).is_err());
    }
}
