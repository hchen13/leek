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
pub mod retry;

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

    // === F2 transcript routing ===
    // These three fields are leek-internal metadata for the
    // `llm_transcripts` archive — they DO NOT travel on the wire.
    // `build_request_body` ignores them; only `CodexClient::chat` reads
    // them when it inserts and finalizes the transcript row.

    /// Session the call belongs to. Used to route the archived row to
    /// the right `llm_transcripts.session_id`.
    pub session_id: String,
    /// Turn the call belongs to.
    pub turn_id: String,
    /// Per-turn monotonic LLM-call sequence number. Spans both the main
    /// agent iterations and the auto-compaction summary call: every code
    /// path that hits `codex.chat` allocates a fresh number, so the
    /// archive has one row per actual provider call.
    ///
    /// This is intentionally a different series from
    /// `turn_metrics.iteration_count`, which counts only main-loop
    /// iterations (compaction is tracked separately there as
    /// `compaction_count`).
    pub iteration: i64,
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
    /// argument deltas, …). Carries no content; the agent loop classifies
    /// these as **heartbeat** — they do NOT reset the idle-timeout budget
    /// (M3.3). Pre-M3.3 every byte reset the timer, so a long silent
    /// reasoning phase that emitted only reasoning_summary lifecycle
    /// frames could hold the loop open indefinitely.
    Ping,
    /// A provider-side web search (M1.9.4). The provider runs the search
    /// itself; the loop does not dispatch or re-inject anything — it only
    /// observes the lifecycle to draw the canvas search card.
    ///
    /// The variant inside `action` says what kind of search activity this
    /// was — a query, an `open_page`, a `find_in_page` — and carries the
    /// fields appropriate to that activity. `None` on the `Start` frame:
    /// the `output_item.added` event has no `action`, so the activity is
    /// unknown until the matching `output_item.done` arrives.
    WebSearch {
        call_id: String,
        phase: WebSearchPhase,
        action: Option<WebSearchAction>,
    },
}

impl LlmEvent {
    /// M3.3: is this event a "substantive" sign of progress that should
    /// reset the idle-timeout budget? Everything except `Ping` is — text
    /// landing, a tool call, a search step, the usage / end-of-response
    /// markers — they all mean the codex backend just gave the loop
    /// something to act on.
    ///
    /// `Ping` covers lifecycle frames (response.in_progress / reasoning
    /// summary deltas / argument deltas / message.added / …) — those
    /// flow steadily during long silent reasoning and used to mask a
    /// genuinely stuck stream. Treating them as heartbeat lets the idle
    /// guard fire when codex stops making real progress, even if it is
    /// still emitting bytes.
    pub fn is_substantive(&self) -> bool {
        !matches!(self, LlmEvent::Ping)
    }
}

/// Lifecycle phase of a provider-side web search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchPhase {
    Started,
    Completed,
}

/// What a `web_search_call` did — derived from the backend's `action.type`
/// (MILESTONES decision 2026-05-20). Each variant carries the fields that
/// activity is meaningful with; `Unknown` keeps the raw `action.type` so an
/// unrecognized activity still renders something instead of crashing.
///
/// The request opts in to per-call results via
/// `include: ["web_search_call.results"]`. That one value covers all
/// activities — `Search` gets a list of `{title, url}`, `OpenPage` gets the
/// page's title and a cleaned content snippet, `FindInPage` gets matching
/// passages.
#[derive(Debug, Clone)]
pub enum WebSearchAction {
    /// `action.type == "search"` — a query over the web.
    Search {
        /// The query the provider actually ran. `None` if absent.
        query: Option<String>,
        /// One entry per `text_result` in the call's `results`. Snippets
        /// are dropped: search cards show titles + URLs only.
        results: Vec<SearchResult>,
    },
    /// `action.type == "open_page"` — the provider opened a URL and read it.
    OpenPage {
        url: String,
        /// The page's title — `None` if absent.
        title: Option<String>,
        /// Cleaned page content from `results[0].snippet`. `None` if
        /// `results` was empty.
        snippet: Option<String>,
    },
    /// `action.type == "find_in_page"` — pattern lookup inside an opened
    /// page. Matches are cleaned snippets from each `results` entry.
    FindInPage {
        url: String,
        pattern: String,
        matches: Vec<String>,
    },
    /// Any other `action.type` — recorded so the UI can show the activity
    /// name without guessing what to render.
    Unknown {
        /// The raw `action.type`.
        kind: String,
    },
}

/// One entry in a `Search` action's `results`.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: Option<String>,
    pub url: String,
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
