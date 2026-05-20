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

use super::{
    ChatRequest, LlmEvent, SearchResult, StopReason, Usage, WebSearchAction, WebSearchPhase,
};

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
        // Opt in to per-call results (MILESTONES decision 2026-05-20). One
        // value, `web_search_call.results`, covers every activity kind:
        // `search` results carry titles + URLs, `open_page` carries the
        // page snippet, `find_in_page` carries matched passages. Without
        // this `include` the search cards show only the activity outline.
        body["include"] = serde_json::json!(["web_search_call.results"]);
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

/// Parse a Responses API SSE byte stream into a flow of `LlmEvent`s.
///
/// Takes the bytes stream (not the `Response`) so that `CodexClient` can
/// tap each chunk into the F2 transcript buffer before handing the stream
/// to the parser — the parser stays a pure bytes-to-events transform.
pub fn parse_sse_stream<S>(bytes_stream: S) -> BoxStream<'static, Result<LlmEvent>>
where
    S: futures::Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>>
        + Send
        + 'static,
{
    Box::pin(try_stream! {
        // Buffer at the byte level: the backend can split a multi-byte UTF-8
        // character (CJK text) across two HTTP chunks, so decoding chunks
        // individually blows up at the boundary. The `\n\n` event delimiter
        // is pure ASCII, so scanning raw bytes is safe; we only decode UTF-8
        // once a whole event has been extracted.
        let mut buf: Vec<u8> = Vec::new();
        futures::pin_mut!(bytes_stream);
        while let Some(chunk) = bytes_stream.next().await {
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
    /// `web_search_call` action — its `type` ("search" / "open_page" /
    /// "find_in_page" / …) selects the variant; the rest of the keys
    /// (`query`, `url`, `pattern`, …) depend on that variant.
    #[serde(default)]
    action: Option<serde_json::Value>,
    /// `web_search_call` results — one `text_result` per entry, populated
    /// when the request opts in via
    /// `include: ["web_search_call.results"]` (MILESTONES decision
    /// 2026-05-20). Empty on the `Start` frame.
    #[serde(default)]
    results: Option<Vec<serde_json::Value>>,
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

/// Normalize a `web_search_call` item into a `WebSearch` event. An item
/// with no id cannot be correlated across its start / completion frames —
/// skip it (→ Ping) rather than emit a dangling artifact. The activity
/// variant is only known on completion; the `Start` frame carries `None`.
fn web_search_event(item: &ItemObject, phase: WebSearchPhase) -> Vec<LlmEvent> {
    let Some(call_id) = item.id.clone().or_else(|| item.call_id.clone()) else {
        return vec![LlmEvent::Ping];
    };
    let action = match phase {
        WebSearchPhase::Started => None,
        WebSearchPhase::Completed => parse_web_search_action(item),
    };
    vec![LlmEvent::WebSearch {
        call_id,
        phase,
        action,
    }]
}

/// Map a completed `web_search_call`'s `action.type` onto a
/// `WebSearchAction` variant (MILESTONES decision 2026-05-20). An item
/// with no `action` returns `None` (nothing to render); an unrecognized
/// `type` becomes `Unknown` so the card still shows the activity name.
fn parse_web_search_action(item: &ItemObject) -> Option<WebSearchAction> {
    let action = item.action.as_ref()?;
    let kind = action.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "search" => {
            let query = pull_query(action);
            let results = parse_search_results(item);
            Some(WebSearchAction::Search { query, results })
        }
        "open_page" => {
            let url = pull_action_str(action, "url").unwrap_or_default();
            let (title, snippet) = parse_first_result(item);
            Some(WebSearchAction::OpenPage {
                url,
                title,
                snippet,
            })
        }
        "find_in_page" => {
            let url = pull_action_str(action, "url").unwrap_or_default();
            let pattern = pull_action_str(action, "pattern").unwrap_or_default();
            let matches = parse_match_snippets(item);
            Some(WebSearchAction::FindInPage {
                url,
                pattern,
                matches,
            })
        }
        other => Some(WebSearchAction::Unknown {
            kind: other.to_string(),
        }),
    }
}

/// Pull the query out of a `search` action — `query` first, else the first
/// of `queries`. Empty strings are treated as absent.
fn pull_query(action: &serde_json::Value) -> Option<String> {
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

/// Pull a string field off an action, dropping empty strings.
fn pull_action_str(action: &serde_json::Value, key: &str) -> Option<String> {
    action
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a `web_search_call`'s `results` into title+url pairs — search
/// cards show titles and URLs only, so the snippet is dropped here. Only
/// `type == "text_result"` entries with a non-empty URL survive.
fn parse_search_results(item: &ItemObject) -> Vec<SearchResult> {
    let Some(arr) = item.results.as_ref() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|r| r.get("type").and_then(|v| v.as_str()) == Some("text_result"))
        .filter_map(|r| {
            let url = r
                .get("url")
                .and_then(|v| v.as_str())
                .filter(|u| !u.is_empty())?;
            let title = r
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(SearchResult {
                title,
                url: url.to_string(),
            })
        })
        .collect()
}

/// First `text_result` of an `open_page` call — its title and the cleaned
/// page snippet. `None`s if the entry or its fields are absent.
fn parse_first_result(item: &ItemObject) -> (Option<String>, Option<String>) {
    let arr = item.results.as_ref();
    let first = arr.and_then(|a| {
        a.iter()
            .find(|r| r.get("type").and_then(|v| v.as_str()) == Some("text_result"))
    });
    let Some(r) = first else {
        return (None, None);
    };
    let title = r
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let snippet = r
        .get("snippet")
        .and_then(|v| v.as_str())
        .map(clean_snippet)
        .filter(|s| !s.is_empty());
    (title, snippet)
}

/// All `text_result` snippets of a `find_in_page` call, cleaned. Empty
/// snippets are dropped.
fn parse_match_snippets(item: &ItemObject) -> Vec<String> {
    let Some(arr) = item.results.as_ref() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|r| r.get("type").and_then(|v| v.as_str()) == Some("text_result"))
        .filter_map(|r| {
            r.get("snippet")
                .and_then(|v| v.as_str())
                .map(clean_snippet)
                .filter(|s| !s.is_empty())
        })
        .collect()
}

/// Strip the codex backend's internal snippet header so the body lands on
/// the canvas as plain page content. Two header shapes appear in practice:
///
/// - `search`: `【turn0search0】 [wordlim: 200] Published: …; Crawled: …;`
/// - `open_page`: `【turn0view0】 [wordlim: 200] Content type: …; Source: open(…); Total lines: N\n`
///   followed by body lines each prefixed with `L<N>: `.
///
/// Each stage is best-effort — a snippet that does not match a stage is
/// left as-is past that point. Inline `【N†...】` citations that appear
/// after a body line's `L<N>: ` prefix are kept (they identify references
/// in the page, not header noise).
fn clean_snippet(raw: &str) -> String {
    let mut s = raw.trim_start();

    // 1. Leading 【...】 citation tag.
    if let Some(rest) = s.strip_prefix('【') {
        if let Some(end) = rest.find('】') {
            s = rest[end + '】'.len_utf8()..].trim_start();
        }
    }

    // 2. [wordlim: N] marker.
    if let Some(after) = strip_wordlim(s) {
        s = after.trim_start();
    }

    // 3. Activity header — search-style or open_page-style. They never
    //    co-occur, so trying one or the other is sufficient.
    if let Some(after) = strip_search_header(s) {
        s = after;
    } else if let Some(after) = strip_open_page_header(s) {
        s = after;
    }

    // 4. Per-line `L<N>: ` prefix — open_page (and presumably find_in_page)
    //    body lines.
    let body: String = s
        .lines()
        .map(strip_line_marker)
        .collect::<Vec<_>>()
        .join("\n");

    body.trim().to_string()
}

fn strip_wordlim(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('[')?;
    let end = rest.find(']')?;
    let inner = rest[..end].trim_start();
    if !inner.starts_with("wordlim:") {
        return None;
    }
    Some(&rest[end + 1..])
}

fn strip_search_header(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("Published:")?;
    let semi = rest.find(';')?;
    let rest = rest[semi + 1..].trim_start();
    let rest = rest.strip_prefix("Crawled:")?;
    let semi = rest.find(';')?;
    Some(&rest[semi + 1..])
}

fn strip_open_page_header(s: &str) -> Option<&str> {
    let rest = s.strip_prefix("Content type:")?;
    let semi = rest.find(';')?;
    let rest = rest[semi + 1..].trim_start();
    let rest = rest.strip_prefix("Source:")?;
    // `Source: open(...)` — the `;` after this field comes AFTER the
    // closing `)`, not the `;` inside the JSON-like blob.
    let paren_close = rest.find(')')?;
    let after_paren = &rest[paren_close + 1..];
    let semi = after_paren.find(';')?;
    let rest = after_paren[semi + 1..].trim_start();
    let rest = rest.strip_prefix("Total lines:")?;
    let nl = rest.find('\n').unwrap_or(rest.len());
    Some(&rest[nl..])
}

fn strip_line_marker(line: &str) -> &str {
    let Some(rest) = line.strip_prefix('L') else {
        return line;
    };
    let digits_end = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_end == 0 {
        return line;
    }
    let rest = &rest[digits_end..];
    let Some(rest) = rest.strip_prefix(':') else {
        return line;
    };
    rest.strip_prefix(' ').unwrap_or(rest)
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
            // Transcript-routing fields are irrelevant to `build_request_body`
            // (it doesn't touch them) — placeholders keep the struct valid.
            session_id: String::new(),
            turn_id: String::new(),
            iteration: 0,
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
            name: "web_fetch".into(),
            description: "Fetch a URL.".into(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let body = build_request_body(&r);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "web_fetch");
    }

    #[test]
    fn body_appends_additional_inputs() {
        let mut r = req();
        r.additional_inputs = vec![
            serde_json::json!({ "type": "function_call", "call_id": "c1", "name": "web_fetch", "arguments": "{}" }),
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
            name: "web_fetch".into(),
            description: "Fetch a URL.".into(),
            parameters: serde_json::json!({ "type": "object" }),
        }];
        let body = build_request_body(&r);
        let tools = body["tools"].as_array().unwrap();
        // The function tool plus the built-in web_search tool.
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|t| t["type"] == "web_search"));
        assert!(tools.iter().any(|t| t["type"] == "function" && t["name"] == "web_fetch"));
    }

    #[test]
    fn body_omits_include_unless_web_search() {
        // `include` only governs web_search_call data — absent without it.
        assert!(build_request_body(&req()).get("include").is_none());
    }

    #[test]
    fn body_adds_include_for_web_search_results() {
        let mut r = req();
        r.web_search = true;
        let body = build_request_body(&r);
        assert_eq!(
            body["include"],
            serde_json::json!(["web_search_call.results"])
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
                   \"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"web_fetch\",\
                   \"arguments\":\"{\\\"url\\\":\\\"x\\\"}\"}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::FunctionCall {
                call_id,
                name,
                arguments,
            }] => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "web_fetch");
                assert_eq!(arguments, "{\"url\":\"x\"}");
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
                action,
            }] => {
                assert_eq!(call_id, "ws_1");
                assert_eq!(*phase, WebSearchPhase::Started);
                // The Start frame has no action — only the matching .done
                // knows what activity this call ran.
                assert!(action.is_none());
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
                action,
            }] => {
                assert_eq!(call_id, "ws_1");
                assert_eq!(*phase, WebSearchPhase::Completed);
                match action {
                    Some(WebSearchAction::Search { query, results }) => {
                        assert_eq!(query.as_deref(), Some("AI capex 2026"));
                        // No `results` on this item — nothing to extract.
                        assert!(results.is_empty());
                    }
                    other => panic!("expected Search variant, got {other:?}"),
                }
            }
            other => panic!("expected WebSearch, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_search_with_results() {
        // With `include: ["web_search_call.results"]`, the completed item
        // carries a `results` array. Empty-URL and non-`text_result` entries
        // drop; titles are trimmed.
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                   \"type\":\"web_search_call\",\"id\":\"ws_7\",\"status\":\"completed\",\
                   \"action\":{\"type\":\"search\",\"query\":\"ai capex\"},\
                   \"results\":[\
                   {\"type\":\"text_result\",\"title\":\" Page A \",\"url\":\"https://a.com/x\",\"snippet\":\"sx\"},\
                   {\"type\":\"text_result\",\"title\":\"Page B\",\"url\":\"https://b.com/y\",\"snippet\":\"sy\"},\
                   {\"type\":\"text_result\",\"title\":\"Page C\",\"url\":\"\",\"snippet\":\"sz\"},\
                   {\"type\":\"other\",\"title\":\"Page D\",\"url\":\"https://d.com/z\",\"snippet\":\"sw\"}]}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::WebSearch {
                call_id,
                phase,
                action,
            }] => {
                assert_eq!(call_id, "ws_7");
                assert_eq!(*phase, WebSearchPhase::Completed);
                match action {
                    Some(WebSearchAction::Search { query, results }) => {
                        assert_eq!(query.as_deref(), Some("ai capex"));
                        assert_eq!(results.len(), 2);
                        assert_eq!(results[0].title.as_deref(), Some("Page A"));
                        assert_eq!(results[0].url, "https://a.com/x");
                        assert_eq!(results[1].title.as_deref(), Some("Page B"));
                        assert_eq!(results[1].url, "https://b.com/y");
                    }
                    other => panic!("expected Search variant, got {other:?}"),
                }
            }
            other => panic!("expected WebSearch, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_open_page_with_cleaned_snippet() {
        // open_page items carry `action: { type, url }` and a single
        // `results` entry whose snippet has a Content-type / Source /
        // Total-lines header followed by `L<N>: …` body lines.
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                   \"type\":\"web_search_call\",\"id\":\"ws_9\",\"status\":\"completed\",\
                   \"action\":{\"type\":\"open_page\",\"url\":\"https://example.com/p\"},\
                   \"results\":[{\"type\":\"text_result\",\"title\":\" Page Title \",\
                   \"url\":\"https://example.com/p\",\
                   \"snippet\":\"【turn0view0】 [wordlim: 200] Content type: text/html; Source: open({}); Total lines: 2\\nL0: Hello\\nL1: World\"}]}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::WebSearch { action, .. }] => match action {
                Some(WebSearchAction::OpenPage {
                    url,
                    title,
                    snippet,
                }) => {
                    assert_eq!(url, "https://example.com/p");
                    assert_eq!(title.as_deref(), Some("Page Title"));
                    assert_eq!(snippet.as_deref(), Some("Hello\nWorld"));
                }
                other => panic!("expected OpenPage variant, got {other:?}"),
            },
            other => panic!("expected WebSearch, got {other:?}"),
        }
    }

    #[test]
    fn parses_web_search_unknown_action_type() {
        // An unrecognized activity is recorded so the card can show the
        // type name instead of guessing fields.
        let raw = "data: {\"type\":\"response.output_item.done\",\"item\":{\
                   \"type\":\"web_search_call\",\"id\":\"ws_x\",\"status\":\"completed\",\
                   \"action\":{\"type\":\"refine_query\"}}}";
        match &parse_one_event(raw).unwrap()[..] {
            [LlmEvent::WebSearch { action, .. }] => match action {
                Some(WebSearchAction::Unknown { kind }) => assert_eq!(kind, "refine_query"),
                other => panic!("expected Unknown variant, got {other:?}"),
            },
            other => panic!("expected WebSearch, got {other:?}"),
        }
    }

    #[test]
    fn output_item_added_for_function_call_is_ping() {
        // Only web_search_call cares about `.added`; a function_call there
        // has empty arguments and must wait for `.done`.
        let raw = "data: {\"type\":\"response.output_item.added\",\"item\":{\
                   \"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"web_fetch\"}}";
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

    // clean_snippet — codex backend's internal snippet header has two
    // shapes (MILESTONES decision 2026-05-20). Each test exercises one,
    // plus a passthrough case and an inline-citation case.

    #[test]
    fn clean_snippet_strips_search_header() {
        let raw = "【turn0search0】 [wordlim: 200] Published: 2 months ago; Crawled: 5 days ago;   Real content here.";
        assert_eq!(clean_snippet(raw), "Real content here.");
    }

    #[test]
    fn clean_snippet_strips_open_page_header_and_line_markers() {
        let raw = "【turn0view0】 [wordlim: 200] Content type: text/html; Source: open({\"ref_id\":\"x\"}); Total lines: 3\nL0: Hello\nL1: \nL2: 【0†Skip to main content】";
        assert_eq!(clean_snippet(raw), "Hello\n\n【0†Skip to main content】");
    }

    #[test]
    fn clean_snippet_passthrough_when_no_header() {
        // A snippet without any recognized header is returned trimmed.
        assert_eq!(clean_snippet("  just text  "), "just text");
    }

    #[test]
    fn clean_snippet_keeps_inline_citations_in_body() {
        // The `L<N>:` prefix drops; the inline 【N†...】 stays — it
        // identifies a reference inside the page, not header noise.
        let raw = "【turn0view0】 [wordlim: 200] Content type: text/html; Source: open(()); Total lines: 1\nL0: see 【7†Foo†bar.com】 here";
        assert_eq!(clean_snippet(raw), "see 【7†Foo†bar.com】 here");
    }
}
