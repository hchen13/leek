//! OpenAI Responses API request body + SSE stream parser.
//!
//! Used by `codex_oauth` (talking to chatgpt.com/backend-api/codex/responses)
//! and will be reused by `openai_api_key` (api.openai.com/v1/responses) later.

use anyhow::{anyhow, Context, Result};
use async_stream::try_stream;
use futures::{stream::BoxStream, StreamExt};
use serde::Deserialize;

use super::{ChatRequest, LlmEvent, Role, StopReason, ToolSpec, Usage, WebSearchAction};
// `ReasoningEffort` is stringified at the call site (`effort.as_str()`),
// so no direct import here.

const DEFAULT_TEXT_VERBOSITY: &str = "low";

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

    let uses_web_search = req
        .tools
        .iter()
        .any(|t| matches!(t, ToolSpec::WebSearch { .. }));
    let tools_json = serialize_tools(&req.tools);
    let prompt_cache_key = prompt_cache_key(req);

    let mut body = serde_json::json!({
        "model": req.model,
        "stream": true,
        "instructions": instructions,
        "input": input,
        "prompt_cache_key": prompt_cache_key,
        // Codex backend disallows server-side response storage; we keep our own
        // history in vault.messages, so this is what we want anyway.
        "store": false,
    });

    if let Some(retention) = prompt_cache_retention(&req.model) {
        body["prompt_cache_retention"] = serde_json::Value::String(retention.to_string());
    }

    if !tools_json.is_empty() {
        body["tools"] = serde_json::Value::Array(tools_json);
    }

    if uses_web_search {
        body["include"] = serde_json::json!(["web_search_call.action.sources"]);
    }

    // Append raw input items after messages (function_call / function_call_output
    // re-injection during multi-turn tool dialog).
    if !req.additional_inputs.is_empty() {
        if let Some(arr) = body["input"].as_array_mut() {
            arr.extend(req.additional_inputs.iter().cloned());
        }
    }

    // Verbosity hint: only gpt-5.x (Responses API, codex backend) supports
    // this field. "low" suppresses padding without changing reasoning budget.
    if req.model.starts_with("gpt-5") {
        body["text"] = serde_json::json!({ "verbosity": DEFAULT_TEXT_VERBOSITY });
    }

    // Reasoning effort for gpt-5/gpt-5.5-style models. Omit when caller
    // didn't ask — backend uses the model's default level.
    if let Some(effort) = req.reasoning_effort {
        body["reasoning"] = serde_json::json!({
            "effort": effort.as_str(),
            "summary": serde_json::Value::Null,
        });
    }

    // Note: `max_output_tokens` is intentionally NOT serialized — the codex
    // backend rejects it as "Unsupported parameter". We'll add it back when
    // the openai_api_key provider lands (platform.openai.com supports it).
    let _ = req.max_output_tokens;
    body
}

fn serialize_tools(tools: &[ToolSpec]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| match t {
            ToolSpec::WebSearch {
                external_web_access,
            } => serde_json::json!({
                "type": "web_search",
                "external_web_access": external_web_access,
            }),
            ToolSpec::Function {
                name,
                description,
                parameters,
            } => serde_json::json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": parameters,
                "strict": false,
            }),
        })
        .collect()
}

fn prompt_cache_key(req: &ChatRequest) -> String {
    let model = req
        .model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect::<String>();
    format!("leek:{model}:main-agent")
}

fn prompt_cache_retention(model: &str) -> Option<&'static str> {
    if model.starts_with("gpt-5") || model.starts_with("gpt-4.1") {
        Some("24h")
    } else {
        None
    }
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
        // Buffer at byte level. The codex backend can split a multi-byte
        // UTF-8 character (e.g. CJK) across two HTTP chunks; decoding
        // chunks individually with `from_utf8` blows up at the boundary
        // (observed during /compact on long Chinese sessions). The blank-
        // line event delimiter is pure ASCII, so scanning bytes is safe;
        // we decode UTF-8 only after we've extracted a whole event.
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.context("reading SSE chunk")?;
            buf.extend_from_slice(&chunk);

            while let Some(idx) = find_event_boundary(&buf) {
                let raw_bytes: Vec<u8> = buf.drain(..idx.next_event_start).collect();
                let raw_event = std::str::from_utf8(&raw_bytes[..idx.end_of_event])
                    .context("non-UTF-8 in SSE event")?;
                for evt in parse_one_event(raw_event)? {
                    yield evt;
                }
            }
        }

        // Flush any complete event left without a trailing blank line.
        if !buf.is_empty() {
            let trailing = std::str::from_utf8(&buf)
                .context("non-UTF-8 in trailing SSE event")?;
            if !trailing.trim().is_empty() {
                for evt in parse_one_event(trailing)? {
                    yield evt;
                }
            }
        }
    })
}

struct EventBoundary {
    end_of_event: usize,
    next_event_start: usize,
}

fn find_event_boundary(buf: &[u8]) -> Option<EventBoundary> {
    if let Some(i) = buf.windows(2).position(|w| w == b"\n\n") {
        return Some(EventBoundary {
            end_of_event: i,
            next_event_start: i + 2,
        });
    }
    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(EventBoundary {
            end_of_event: i,
            next_event_start: i + 4,
        });
    }
    None
}

#[derive(Deserialize)]
struct DataEnvelope {
    #[serde(rename = "type")]
    type_: Option<String>,
    /// Owned String (not borrowed `&str`): the codex backend ships text
    /// containing escapes like `\n`, which serde_json can't zero-copy borrow.
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    response: Option<ResponseObject>,
    #[serde(default)]
    item: Option<ItemObject>,
}

#[derive(Deserialize)]
struct ItemObject {
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    action: Option<serde_json::Value>,
    // function_call fields — present only when type="function_call"
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    /// Wire form is a string containing JSON (per codex-rs ResponseItem doc).
    #[serde(default)]
    arguments: Option<String>,
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

    let env: DataEnvelope = serde_json::from_str(&data)
        .with_context(|| format!("parsing SSE data line as JSON: {}", truncate(&data, 200)))?;

    match env.type_.as_deref() {
        Some("response.output_text.delta") => {
            let text = env
                .delta
                .ok_or_else(|| anyhow!("output_text.delta event missing 'delta'"))?;
            Ok(vec![LlmEvent::TextDelta { text }])
        }
        // Server-side web_search lifecycle + client-side function_call dispatch.
        // `output_item.added` fires on start, `output_item.done` fires when the
        // item is complete. Both wrap many item kinds — dispatch on item.type.
        Some("response.output_item.added") | Some("response.output_item.done") => {
            let is_done = env.type_.as_deref() == Some("response.output_item.done");
            let Some(item) = env.item else {
                return Ok(Vec::new());
            };
            match item.type_.as_deref() {
                Some("web_search_call") => {
                    let action = item.action.as_ref().and_then(parse_web_search_action);
                    let status = item.status.unwrap_or_default();
                    Ok(vec![LlmEvent::WebSearchCall { status, action }])
                }
                // Function call: arguments are accumulated server-side and
                // delivered complete on `output_item.done`. The `added` event
                // typically has empty arguments — we skip it to avoid emitting
                // a partial FunctionCall the agent can't dispatch.
                Some("function_call") if is_done => {
                    let call_id = item
                        .call_id
                        .ok_or_else(|| anyhow!("function_call done event missing 'call_id'"))?;
                    let name = item
                        .name
                        .ok_or_else(|| anyhow!("function_call done event missing 'name'"))?;
                    let arguments = item.arguments.unwrap_or_default();
                    Ok(vec![LlmEvent::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    }])
                }
                _ => Ok(Vec::new()),
            }
        }
        // Argument deltas — useful only for "tool call streaming…" UX. We
        // accumulate in the agent layer if/when we surface that progress;
        // for now silently drop, since `output_item.done` carries the full
        // arguments string anyway.
        Some("response.function_call_arguments.delta")
        | Some("response.function_call_arguments.done")
        | Some("response.custom_tool_call_input.delta") => Ok(Vec::new()),
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
        Some("response.failed") | Some("response.error") | Some("error") => Err(anyhow!(
            "provider returned error event: {}",
            truncate(&data, 500)
        )),
        // Lifecycle events we recognize but don't emit anything for (P1 scope)
        Some(_) | None => Ok(Vec::new()),
    }
}

fn parse_web_search_action(v: &serde_json::Value) -> Option<WebSearchAction> {
    let obj = v.as_object()?;
    match obj.get("type").and_then(|t| t.as_str())? {
        "search" => {
            let query = obj
                .get("query")
                .and_then(|q| q.as_str())
                .map(String::from)
                .or_else(|| {
                    // Some backends pass `queries: ["..."]` instead of singular.
                    obj.get("queries")
                        .and_then(|q| q.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|s| s.as_str())
                        .map(String::from)
                })
                .unwrap_or_default();
            let mut queries: Vec<String> = obj
                .get("queries")
                .and_then(|q| q.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            if queries.is_empty() && !query.is_empty() {
                queries.push(query.clone());
            }
            let sources = obj
                .get("sources")
                .and_then(|q| q.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.get("url").and_then(|url| url.as_str()))
                        .filter(|s| !s.trim().is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            Some(WebSearchAction::Search {
                query,
                queries,
                sources,
            })
        }
        "open_page" => {
            let url = obj
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string();
            Some(WebSearchAction::OpenPage { url })
        }
        "find_in_page" => {
            let url = obj
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string();
            let pattern = obj
                .get("pattern")
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string();
            Some(WebSearchAction::FindInPage { url, pattern })
        }
        _ => Some(WebSearchAction::Other),
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

    #[test]
    fn parses_web_search_call_in_progress_with_query() {
        let raw = "data: {\"type\":\"response.output_item.added\",\"item\":{\
                       \"id\":\"ws_1\",\"type\":\"web_search_call\",\"status\":\"in_progress\",\
                       \"action\":{\"type\":\"search\",\"query\":\"NVDA Q1\"}}}";
        let evts = parse_one_event(raw).unwrap();
        assert_eq!(evts.len(), 1);
        match &evts[0] {
            LlmEvent::WebSearchCall { status, action } => {
                assert_eq!(status, "in_progress");
                match action {
                    Some(super::WebSearchAction::Search {
                        query,
                        queries,
                        sources,
                    }) => {
                        assert_eq!(query, "NVDA Q1");
                        assert_eq!(queries, &vec!["NVDA Q1".to_string()]);
                        assert!(sources.is_empty());
                    }
                    other => panic!("expected Search action, got {other:?}"),
                }
            }
            other => panic!("expected WebSearchCall, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_call_with_query_batch() {
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                       \"id\":\"ws_1\",\"type\":\"web_search_call\",\"status\":\"completed\",\
                       \"action\":{\"type\":\"search\",\"query\":\"CRCL news\",\
                       \"queries\":[\"CRCL news\",\"Circle latest news\"],\
                       \"sources\":[{\"type\":\"url\",\"url\":\"https://www.circle.com/news\"}]}}}";
        let evts = parse_one_event(raw).unwrap();
        assert_eq!(evts.len(), 1);
        match &evts[0] {
            LlmEvent::WebSearchCall { status, action } => {
                assert_eq!(status, "completed");
                match action {
                    Some(super::WebSearchAction::Search {
                        query,
                        queries,
                        sources,
                    }) => {
                        assert_eq!(query, "CRCL news");
                        assert_eq!(
                            queries,
                            &vec!["CRCL news".to_string(), "Circle latest news".to_string(),]
                        );
                        assert_eq!(sources, &vec!["https://www.circle.com/news".to_string()]);
                    }
                    other => panic!("expected Search action, got {other:?}"),
                }
            }
            other => panic!("expected WebSearchCall, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_call_completed_with_open_page() {
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                       \"id\":\"ws_2\",\"type\":\"web_search_call\",\"status\":\"completed\",\
                       \"action\":{\"type\":\"open_page\",\"url\":\"https://example.com/a\"}}}";
        let evts = parse_one_event(raw).unwrap();
        assert_eq!(evts.len(), 1);
        match &evts[0] {
            LlmEvent::WebSearchCall { status, action } => {
                assert_eq!(status, "completed");
                match action {
                    Some(super::WebSearchAction::OpenPage { url }) => {
                        assert_eq!(url, "https://example.com/a")
                    }
                    other => panic!("expected OpenPage, got {other:?}"),
                }
            }
            other => panic!("expected WebSearchCall, got {other:?}"),
        }
    }

    #[test]
    fn ignores_output_item_for_non_web_search() {
        // output_item.added/done also wraps text/reasoning items — we should
        // emit nothing for those (text comes from output_text.delta instead).
        let raw = "data: {\"type\":\"response.output_item.added\",\"item\":{\
                       \"type\":\"reasoning\",\"id\":\"r1\"}}";
        assert!(parse_one_event(raw).unwrap().is_empty());
    }

    #[test]
    fn build_request_body_serializes_tools_when_present() {
        use super::super::{ChatMessage, ChatRequest, Role, ToolSpec};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: vec![ToolSpec::WebSearch {
                external_web_access: true,
            }],
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "web_search");
        assert_eq!(tools[0]["external_web_access"], true);
        assert_eq!(
            body["include"],
            serde_json::json!(["web_search_call.action.sources"])
        );
        assert_eq!(body["prompt_cache_key"], "leek:gpt-5.5:main-agent");
        assert_eq!(body["prompt_cache_retention"], "24h");
    }

    #[test]
    fn build_request_body_omits_tools_when_empty() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn parses_function_call_done() {
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                       \"id\":\"fc_1\",\"type\":\"function_call\",\
                       \"call_id\":\"call_42\",\"name\":\"web_fetch\",\
                       \"arguments\":\"{\\\"url\\\":\\\"https://x.com\\\"}\"}}";
        let evts = parse_one_event(raw).unwrap();
        assert_eq!(evts.len(), 1);
        match &evts[0] {
            LlmEvent::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_42");
                assert_eq!(name, "web_fetch");
                assert_eq!(arguments, "{\"url\":\"https://x.com\"}");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn function_call_added_is_silent() {
        // The `added` event fires before arguments are streamed; we skip it
        // to avoid double-dispatching tools.
        let raw = "data: {\"type\":\"response.output_item.added\",\"item\":{\
                       \"id\":\"fc_1\",\"type\":\"function_call\",\
                       \"call_id\":\"c1\",\"name\":\"web_fetch\",\"arguments\":\"\"}}";
        assert!(parse_one_event(raw).unwrap().is_empty());
    }

    #[test]
    fn function_call_arguments_delta_silent() {
        let raw = "data: {\"type\":\"response.function_call_arguments.delta\",\
                       \"item_id\":\"fc_1\",\"delta\":\"{\\\"u\"}";
        assert!(parse_one_event(raw).unwrap().is_empty());
    }

    #[test]
    fn build_request_body_serializes_function_tool() {
        use super::super::{ChatMessage, ChatRequest, Role, ToolSpec};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: vec![ToolSpec::Function {
                name: "web_fetch".into(),
                description: "Fetch a URL.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"url": {"type": "string"}},
                    "required": ["url"],
                }),
            }],
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "web_fetch");
        assert_eq!(tools[0]["description"], "Fetch a URL.");
        assert_eq!(tools[0]["parameters"]["required"][0], "url");
    }

    #[test]
    fn event_boundary_finds_lf_in_byte_buffer() {
        // "data: 中文\n\nremaining" — boundary should land cleanly even
        // though the body contains 3-byte CJK characters. UTF-8 \n (0x0A)
        // never appears inside continuation bytes (0x80-0xBF), so byte
        // search is safe.
        let buf = b"data: \xe4\xb8\xad\xe6\x96\x87\n\nremaining";
        let idx = super::find_event_boundary(buf).unwrap();
        let event = std::str::from_utf8(&buf[..idx.end_of_event]).unwrap();
        assert_eq!(event, "data: 中文");
        assert_eq!(&buf[idx.next_event_start..], b"remaining");
    }

    #[test]
    fn event_boundary_returns_none_for_partial_multibyte_at_end() {
        // chunk ends mid-character — no event boundary yet, caller
        // must accumulate more bytes before decoding. This is exactly
        // the case that broke the old String-based pipeline.
        let buf = b"data: hello\xe4\xb8"; // truncated 中
        assert!(super::find_event_boundary(buf).is_none());
    }

    #[test]
    fn build_request_body_serializes_reasoning_effort() {
        use super::super::{ChatMessage, ChatRequest, ReasoningEffort, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "summarize this".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: Some(ReasoningEffort::Minimal),
        };
        let body = build_request_body(&req);
        assert_eq!(body["reasoning"]["effort"], "minimal");
        assert!(body["reasoning"]["summary"].is_null());
    }

    #[test]
    fn build_request_body_omits_reasoning_when_none() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn build_request_body_defaults_text_verbosity_to_low_for_gpt5() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "briefly explain".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        assert_eq!(body["text"]["verbosity"], DEFAULT_TEXT_VERBOSITY);
    }

    #[test]
    fn build_request_body_omits_text_verbosity_for_non_gpt5() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            system: None,
            model: "gpt-4o".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        assert!(body.get("text").is_none());
    }

    #[test]
    fn build_request_body_cache_key_ignores_chat_history() {
        use super::super::{ChatMessage, ChatRequest, Role, ToolSpec};
        let mk = |content: &str| ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: content.into(),
            }],
            system: Some("stable instructions".into()),
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: vec![ToolSpec::Function {
                name: "web_fetch".into(),
                description: "Fetch a URL.".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let first = build_request_body(&mk("first question"));
        let second = build_request_body(&mk("second question"));

        assert_eq!(first["prompt_cache_key"], second["prompt_cache_key"]);
    }

    #[test]
    fn build_request_body_cache_key_is_stable_across_static_prefix_changes() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let mk = |system: &str| ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "same question".into(),
            }],
            system: Some(system.into()),
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let first = build_request_body(&mk("stable instructions"));
        let second = build_request_body(&mk("changed instructions"));

        assert_eq!(first["prompt_cache_key"], second["prompt_cache_key"]);
    }

    #[test]
    fn build_request_body_omits_24h_cache_retention_for_unsupported_model_family() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            system: None,
            model: "gpt-4o".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        assert!(body.get("prompt_cache_retention").is_none());
    }

    #[test]
    fn build_request_body_appends_additional_inputs() {
        use super::super::{ChatMessage, ChatRequest, Role};
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: "fetch".into(),
            }],
            system: None,
            model: "gpt-5.5".into(),
            max_output_tokens: None,
            tools: Vec::new(),
            additional_inputs: vec![
                serde_json::json!({
                    "type": "function_call",
                    "call_id": "c1",
                    "name": "web_fetch",
                    "arguments": "{\"url\":\"https://x\"}"
                }),
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": "c1",
                    "output": "OK"
                }),
            ],
            reasoning_effort: None,
        };
        let body = build_request_body(&req);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
    }
}
