//! LLM access — the codex backend, reached over the OpenAI Responses API.
//!
//! There is exactly one concrete provider (`CodexClient`) and no provider
//! trait: per docs/ARCHITECTURE.md §2 ("不抽象 LLM provider") the abstraction
//! is added only when a real second provider appears. This
//! module is the wire layer — request/response types, the request-body
//! builder, the SSE parser, OAuth, and the client itself.

pub mod codex;
pub mod oauth;
pub mod pricing;
pub mod responses;

/// One chat request to the model. `system` is the system prompt
/// (Responses API `instructions`); `messages` is the conversation;
/// `additional_inputs` are raw Responses-API input items appended after
/// the messages — the agent loop uses them to re-inject prior `function_call`
/// / `function_call_output` pairs and per-iteration developer hints.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub additional_inputs: Vec<serde_json::Value>,
    /// `xhigh` / `high` / `medium` / `low` / `minimal`; `None` = backend default.
    pub reasoning_effort: Option<String>,
    /// `low` / `medium` / `high`; `None` = backend default.
    pub verbosity: Option<String>,
    /// Offer the provider-side `web_search` tool (M1.9.4). When `true`,
    /// `build_request_body` appends the built-in `web_search` tool; the
    /// provider runs searches server-side and the loop normalizes the
    /// search lifecycle into `search_lifecycle` canvas events. Off unless
    /// `LEEK_WEB_SEARCH` is set — the capability stays opt-in until it is
    /// verified live against the codex backend.
    pub web_search: bool,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Harness-injected context — neither a user nor a model utterance.
    /// Used for the auto-compaction summary block (see `agent::compaction`).
    Developer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Developer => "developer",
        }
    }
}

/// A client-side function tool offered to the model. Passed verbatim into
/// the Responses API `tools` array; the model emits `function_call` items
/// which the agent loop dispatches and answers with `function_call_output`.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the argument object.
    pub parameters: serde_json::Value,
}

/// A normalized event from the model's streamed response.
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Incremental assistant text.
    TextDelta { text: String },
    /// The model asked to call a client-side tool. `arguments` is a JSON
    /// string (parse before use).
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Token accounting for the response (arrives once, near the end).
    Usage(Usage),
    /// The response finished.
    MessageEnd { stop_reason: StopReason },
    /// A recognized-but-uninteresting SSE event (reasoning lifecycle,
    /// argument deltas, …). Carries no content; the agent loop only uses
    /// it to keep the idle-timeout timer alive while the model is working
    /// silently.
    Ping,
    /// A provider-side web search (M1.9.4). The provider runs the search
    /// itself; the loop does not dispatch or re-inject anything — it only
    /// observes the lifecycle to draw the canvas search card.
    WebSearch {
        call_id: String,
        phase: WebSearchPhase,
        /// The search query — reported once the provider has it (on
        /// completion); `None` while the search is still starting.
        query: Option<String>,
        /// Resolved source URLs for this search, parsed from the backend's
        /// `web_search_call.action.sources` (REQUIREMENTS §4.3, MILESTONES
        /// decision 2026-05-19). Empty on the `Start` frame; populated on
        /// `Completed` — the request opts in to this data via `include`.
        sources: Vec<String>,
    },
}

/// Lifecycle phase of a provider-side web search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchPhase {
    Started,
    Completed,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Cached prompt tokens — billed at the cheaper cache rate.
    pub cache_read_tokens: u32,
}

/// Why a single response finished. The agent loop maps this to the turn's
/// `stop_reason` only when the turn ends naturally (no tool calls, no guard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
}
