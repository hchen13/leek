//! OpenAI Responses API — request-body builder and SSE stream parser.
//!
//! The codex backend (`chatgpt.com/backend-api/codex/responses`) speaks the
//! OpenAI Responses API. This module turns a `ChatRequest` into the request
//! JSON and turns the streamed `text/event-stream` response into a flow of
//! normalized `LlmEvent`s.

use anyhow::{anyhow, Context, Result};
use async_stream::try_stream;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde::Deserialize;

use super::{ChatRequest, LlmEvent, StopReason, Usage, WebSearchPhase};

/// Build the Responses API request body for `req`.
///
/// Notable choices:
/// - `store: false` — the codex backend disallows server-side response
///   storage anyway, and leek keeps its own history in `vault.messages`.
/// - no `max_output_tokens` — codex-rs's request struct has no such field;
///   the provider's per-model default is trusted (ARCHITECTURE §12.3,
///   MILESTONES decision 2026-05-09).
pub fn build_request_body(req: &ChatRequest) -> serde_json::Value {
    let input: Vec<serde_json::Value> = req
        .messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role.as_str(), "content": m.content }))
        .collect();

    let mut body = serde_json::json!({
        "model": req.model,
        "stream": true,
        "instructions": req.system,
        "input": input,
        "store": false,
    });

    let mut tools: Vec<serde_json::Value> = req
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
                "strict": false,
            })
        })
        .collect();
    // Provider-side web search (M1.9.4): a built-in tool, not a client
    // function tool — the provider runs it server-side. The minimal
    // `{type:"web_search"}` form is the OpenAI Responses API shape; leek
    // keeps it opt-in (see `ChatRequest.web_search`).
    if req.web_search {
        tools.push(serde_json::json!({ "type": "web_search" }));
        // Opt in to per-search source data: the backend then attaches each
        // search's resolved sources to its `web_search_call.action.sources`
        // (MILESTONES decision 2026-05-19). Without this `include` the search
        // card stays sourceless — codex does not surface them otherwise.
        body["include"] = serde_json::json!(["web_search_call.action.sources"]);
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }

    // Raw input items appended after the conversation — the agent loop's
    // re-injected function_call / function_call_output pairs and any
    // per-iteration developer hint.
    if !req.additional_inputs.is_empty() {
        if let Some(arr) = body["input"].as_array_mut() {
            arr.extend(req.additional_inputs.iter().cloned());
        }
    }

    if let Some(effort) = &req.reasoning_effort {
        body["reasoning"] =
            serde_json::json!({ "effort": effort, "summary": serde_json::Value::Null });
    }
    if let Some(verbosity) = &req.verbosity {
        body["text"] = serde_json::json!({ "verbosity": verbosity });
    }

    body
}

/// Parse a Responses API SSE stream into a flow of `LlmEvent`s.
pub fn parse_sse_stream(resp: reqwest::Response) -> BoxStream<'static, Result<LlmEvent>> {
    let mut bytes = resp.bytes_stream();

    Box::pin(try_stream! {
        // Buffer at the byte level: the backend can split a multi-byte UTF-8
        // character (CJK text) across two HTTP chunks, so decoding chunks
        // individually blows up at the boundary. The `\n\n` event delimiter
        // is pure ASCII, so scanning raw bytes is safe; we only decode UTF-8
        // once a whole event has been extracted.
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.context("reading SSE chunk")?;
            buf.extend_from_slice(&chunk);

            while let Some(b) = find_event_boundary(&buf) {
                let raw: Vec<u8> = buf.drain(..b.next_start).collect();
                let event = std::str::from_utf8(&raw[..b.event_end])
                    .context("non-UTF-8 in SSE event")?;
                for evt in parse_one_event(event)? {
                    yield evt;
                }
            }
        }

        // Flush a trailing event with no closing blank line.
        if !buf.is_empty() {
            if let Ok(trailing) = std::str::from_utf8(&buf) {
                if !trailing.trim().is_empty() {
                    for evt in parse_one_event(trailing)? {
                        yield evt;
                    }
                }
            }
        }
    })
}

struct Boundary {
    event_end: usize,
    next_start: usize,
}

fn find_event_boundary(buf: &[u8]) -> Option<Boundary> {
    if let Some(i) = buf.windows(2).position(|w| w == b"\n\n") {
        return Some(Boundary {
            event_end: i,
            next_start: i + 2,
        });
    }
    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(Boundary {
            event_end: i,
            next_start: i + 4,
        });
    }
    None
}

#[derive(Deserialize)]
struct DataEnvelope {
    #[serde(rename = "type")]
    type_: Option<String>,
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
    name: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    /// Wire form is a JSON string (codex-rs `ResponseItem`).
    #[serde(default)]
    arguments: Option<String>,
    /// `web_search_call` items carry their id in `id`, not `call_id`.
    #[serde(default)]
    id: Option<String>,
    /// `web_search_call` action — `{type:"search", query, queries, sources}`.
    #[serde(default)]
    action: Option<serde_json::Value>,
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

/// Parse one raw SSE event block into zero or more `LlmEvent`s. Any
/// recognized-but-untranslated event yields a single `Ping` so the loop's
/// idle timer sees the model is still working.
fn parse_one_event(raw: &str) -> Result<Vec<LlmEvent>> {
    // Concatenate `data:` lines per the SSE spec; ignore `event:` / `id:`.
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(payload) = line.trim_end().strip_prefix("data:") {
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
        .with_context(|| format!("parsing SSE data as JSON: {}", truncate(&data, 200)))?;

    match env.type_.as_deref() {
        Some("response.output_text.delta") => {
            let text = env
                .delta
                .ok_or_else(|| anyhow!("output_text.delta missing 'delta'"))?;
            Ok(vec![LlmEvent::TextDelta { text }])
        }
        // Function-call items: arguments are accumulated server-side and
        // delivered complete on `output_item.done`. A `web_search_call`
        // item is a provider-side search (M1.9.4) — `.done` completes it
        // with the query and resolved sources in `action`.
        Some("response.output_item.done") => {
            let Some(item) = env.item else {
                return Ok(vec![LlmEvent::Ping]);
            };
            match item.type_.as_deref() {
                Some("function_call") => {
                    let call_id = item
                        .call_id
                        .ok_or_else(|| anyhow!("function_call done missing 'call_id'"))?;
                    let name = item
                        .name
                        .ok_or_else(|| anyhow!("function_call done missing 'name'"))?;
                    Ok(vec![LlmEvent::FunctionCall {
                        call_id,
                        name,
                        arguments: item.arguments.unwrap_or_default(),
                    }])
                }
                Some("web_search_call") => Ok(web_search_event(&item, WebSearchPhase::Completed)),
                // A completed assistant message — its text has already
                // streamed as deltas, so the item itself adds nothing.
                Some("message") => Ok(vec![LlmEvent::Ping]),
                // The `added` form of a function_call has empty arguments —
                // skip it (→ Ping) and dispatch off the complete `.done`.
                _ => Ok(vec![LlmEvent::Ping]),
            }
        }
        // A `web_search_call` item appearing means a search has started.
        Some("response.output_item.added") => {
            match env.item.filter(|i| i.type_.as_deref() == Some("web_search_call")) {
                Some(item) => Ok(web_search_event(&item, WebSearchPhase::Started)),
                None => Ok(vec![LlmEvent::Ping]),
            }
        }
        Some("response.completed") => {
            let response = env
                .response
                .ok_or_else(|| anyhow!("response.completed missing 'response'"))?;
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
                }),
                LlmEvent::MessageEnd { stop_reason: stop },
            ])
        }
        Some("response.failed") | Some("response.error") | Some("error") => Err(anyhow!(
            "provider returned an error event: {}",
            truncate(&data, 400)
        )),
        // Everything else (lifecycle, reasoning summary, argument deltas):
        // recognized, no content — a heartbeat for the idle timer.
        Some(_) | None => Ok(vec![LlmEvent::Ping]),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Normalize a `web_search_call` item into a `WebSearch` event. An item with
/// no id cannot be correlated across its start / completion frames — skip it
/// (→ Ping) rather than emit a dangling artifact.
fn web_search_event(item: &ItemObject, phase: WebSearchPhase) -> Vec<LlmEvent> {
    let Some(call_id) = item.id.clone().or_else(|| item.call_id.clone()) else {
        return vec![LlmEvent::Ping];
    };
    let query = item.action.as_ref().and_then(search_query);
    let sources = search_sources(item.action.as_ref());
    vec![LlmEvent::WebSearch {
        call_id,
        phase,
        query,
        sources,
    }]
}

/// Pull the query out of a `web_search_call` action
/// (`{type:"search", query, queries}`) — `query` first, else the first of
/// `queries`.
fn search_query(action: &serde_json::Value) -> Option<String> {
    let from = |v: Option<&serde_json::Value>| {
        v.and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    from(action.get("query")).or_else(|| {
        from(action
            .get("queries")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first()))
    })
}

/// Pull resolved source URLs out of a `web_search_call` action. The codex
/// backend attaches them as `action.sources: [{type:"url", url:"…"}, …]`
/// once the request opts in via `include: ["web_search_call.action.sources"]`
/// (MILESTONES decision 2026-05-19). Only `type == "url"` entries with a
/// non-empty URL are kept; a `Start`-frame action carries no `sources`.
fn search_sources(action: Option<&serde_json::Value>) -> Vec<String> {
    let Some(arr) = action
        .and_then(|a| a.get("sources"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter()
        .filter(|s| s.get("type").and_then(|v| v.as_str()) == Some("url"))
        .filter_map(|s| {
            s.get("url")
                .and_then(|v| v.as_str())
                .filter(|u| !u.is_empty())
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, Role, ToolSpec};

    fn req() -> ChatRequest {
        ChatRequest {
            model: "gpt-5.5".into(),
            system: "sys".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: None,
            verbosity: None,
            web_search: false,
        }
    }

    #[test]
    fn body_has_no_max_output_tokens() {
        let body = build_request_body(&req());
        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["instructions"], "sys");
    }

    #[test]
    fn body_serializes_function_tool() {
        let mut r = req();
        r.tools = vec![ToolSpec {
            name: "echo".into(),
            description: "Echo text.".into(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let body = build_request_body(&r);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "echo");
    }

    #[test]
    fn body_appends_additional_inputs() {
        let mut r = req();
        r.additional_inputs = vec![
            serde_json::json!({ "type": "function_call", "call_id": "c1", "name": "echo", "arguments": "{}" }),
            serde_json::json!({ "type": "function_call_output", "call_id": "c1", "output": "ok" }),
        ];
        let body = build_request_body(&r);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
    }

    #[test]
    fn body_emits_reasoning_and_verbosity() {
        let mut r = req();
        r.reasoning_effort = Some("xhigh".into());
        r.verbosity = Some("low".into());
        let body = build_request_body(&r);
        assert_eq!(body["reasoning"]["effort"], "xhigh");
        assert!(body["reasoning"]["summary"].is_null());
        assert_eq!(body["text"]["verbosity"], "low");
    }

    #[test]
    fn body_omits_web_search_unless_enabled() {
        // Default request: no tools at all.
        assert!(build_request_body(&req()).get("tools").is_none());
    }

    #[test]
    fn body_appends_web_search_tool() {
        let mut r = req();
        r.web_search = true;
        r.tools = vec![ToolSpec {
            name: "echo".into(),
            description: "Echo.".into(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let body = build_request_body(&r);
        let tools = body["tools"].as_array().unwrap();
        // The function tool plus the built-in web_search tool.
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|t| t["type"] == "web_search"));
        assert!(tools.iter().any(|t| t["type"] == "function" && t["name"] == "echo"));
    }

    #[test]
    fn body_omits_include_unless_web_search() {
        // `include` only governs web_search_call data — absent without it.
        assert!(build_request_body(&req()).get("include").is_none());
    }

    #[test]
    fn body_adds_include_for_web_search_sources() {
        let mut r = req();
        r.web_search = true;
        let body = build_request_body(&r);
        assert_eq!(
            body["include"],
            serde_json::json!(["web_search_call.action.sources"])
        );
    }

    #[test]
    fn parses_text_delta() {
        let raw = "event: response.output_text.delta\n\
                   data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::TextDelta { text }] => assert_eq!(text, "Hi"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parses_completed_with_usage() {
        let raw = "data: {\"type\":\"response.completed\",\"response\":{\
                   \"status\":\"completed\",\"usage\":{\"input_tokens\":10,\
                   \"output_tokens\":5,\"input_tokens_details\":{\"cached_tokens\":3}}}}";
        let evts = parse_one_event(raw).unwrap();
        assert_eq!(evts.len(), 2);
        match &evts[0] {
            LlmEvent::Usage(u) => {
                assert_eq!(u.input_tokens, 10);
                assert_eq!(u.output_tokens, 5);
                assert_eq!(u.cache_read_tokens, 3);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(
            evts[1],
            LlmEvent::MessageEnd {
                stop_reason: StopReason::EndTurn
            }
        ));
    }

    #[test]
    fn parses_function_call_done() {
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                   \"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"echo\",\
                   \"arguments\":\"{\\\"text\\\":\\\"x\\\"}\"}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::FunctionCall {
                call_id,
                name,
                arguments,
            }] => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "echo");
                assert_eq!(arguments, "{\"text\":\"x\"}");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_started() {
        let raw = "data: {\"type\":\"response.output_item.added\",\"item\":{\
                   \"type\":\"web_search_call\",\"id\":\"ws_1\",\"status\":\"in_progress\"}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::WebSearch {
                call_id,
                phase,
                query,
                sources,
            }] => {
                assert_eq!(call_id, "ws_1");
                assert_eq!(*phase, WebSearchPhase::Started);
                assert!(query.is_none());
                // The `Start` frame has no `action`, hence no sources.
                assert!(sources.is_empty());
            }
            other => panic!("expected WebSearch, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_completed_with_query() {
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                   \"type\":\"web_search_call\",\"id\":\"ws_1\",\"status\":\"completed\",\
                   \"action\":{\"type\":\"search\",\"query\":\"AI capex 2026\"}}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::WebSearch {
                call_id,
                phase,
                query,
                sources,
            }] => {
                assert_eq!(call_id, "ws_1");
                assert_eq!(*phase, WebSearchPhase::Completed);
                assert_eq!(query.as_deref(), Some("AI capex 2026"));
                // No `sources` array on this action — none parsed.
                assert!(sources.is_empty());
            }
            other => panic!("expected WebSearch, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_sources_from_action() {
        // With the `include` opt-in, a completed web_search_call's action
        // carries the resolved sources. Empty-URL and non-`url` entries drop.
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                   \"type\":\"web_search_call\",\"id\":\"ws_7\",\"status\":\"completed\",\
                   \"action\":{\"type\":\"search\",\"query\":\"ai capex\",\"sources\":[\
                   {\"type\":\"url\",\"url\":\"https://a.com/x\"},\
                   {\"type\":\"url\",\"url\":\"https://b.com/y\"},\
                   {\"type\":\"url\",\"url\":\"\"},\
                   {\"type\":\"other\",\"url\":\"https://c.com/z\"}]}}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::WebSearch {
                call_id,
                phase,
                query,
                sources,
            }] => {
                assert_eq!(call_id, "ws_7");
                assert_eq!(*phase, WebSearchPhase::Completed);
                assert_eq!(query.as_deref(), Some("ai capex"));
                assert_eq!(sources.len(), 2);
                assert_eq!(sources[0], "https://a.com/x");
                assert_eq!(sources[1], "https://b.com/y");
            }
            other => panic!("expected WebSearch with sources, got {other:?}"),
        }
    }

    #[test]
    fn output_item_added_for_function_call_is_ping() {
        // Only web_search_call cares about `.added`; a function_call there
        // has empty arguments and must wait for `.done`.
        let raw = "data: {\"type\":\"response.output_item.added\",\"item\":{\
                   \"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"echo\"}}";
        assert!(matches!(parse_one_event(raw).unwrap()[..], [LlmEvent::Ping]));
    }

    #[test]
    fn unknown_event_is_ping() {
        let raw = "data: {\"type\":\"response.in_progress\"}";
        assert!(matches!(
            parse_one_event(raw).unwrap()[..],
            [LlmEvent::Ping]
        ));
    }

    #[test]
    fn done_marker_yields_nothing() {
        assert!(parse_one_event("data: [DONE]").unwrap().is_empty());
    }

    #[test]
    fn failed_event_is_error() {
        let raw = "data: {\"type\":\"response.failed\",\"error\":{}}";
        assert!(parse_one_event(raw).is_err());
    }

    #[test]
    fn event_boundary_survives_split_multibyte() {
        // "data: 中文\n\nrest" — boundary is found even with 3-byte CJK.
        let buf = b"data: \xe4\xb8\xad\xe6\x96\x87\n\nrest";
        let b = find_event_boundary(buf).unwrap();
        assert_eq!(
            std::str::from_utf8(&buf[..b.event_end]).unwrap(),
            "data: 中文"
        );
        assert_eq!(&buf[b.next_start..], b"rest");
    }

    #[test]
    fn event_boundary_none_for_partial_multibyte() {
        // Chunk ends mid-character — no boundary yet.
        assert!(find_event_boundary(b"data: hi\xe4\xb8").is_none());
    }
}
