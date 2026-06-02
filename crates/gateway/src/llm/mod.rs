//! LLM provider abstraction.
//!
//! See `design/p1-spec/llm-provider.md` for the trait contract and the
//! three P1 providers. This module currently implements `codex_oauth` only;
//! `anthropic_api_key` and `openai_api_key` will land later.

pub mod codex_oauth;
pub mod model_limits;
pub mod openai_responses;

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system: Option<String>,
    pub model: String,
    pub session_id: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub tools: Vec<ToolSpec>,
    /// Raw input items appended after `messages` — used by the agent loop to
    /// inject prior-turn `function_call` and `function_call_output` items so
    /// the model has a complete view of the multi-turn tool dialog. Each item
    /// must already match the OpenAI Responses API input shape (see
    /// codex-rs/protocol/src/models.rs ResponseItem).
    pub additional_inputs: Vec<serde_json::Value>,
    /// Reasoning effort for models that support it (gpt-5/gpt-5.5/...).
    /// `None` means use the model's backend default.
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// Mirrors the gpt-5.5 reasoning effort values leek uses today.
#[derive(Debug, Clone, Copy)]
pub enum ReasoningEffort {
    Low,
    #[allow(dead_code)]
    Medium,
    #[allow(dead_code)]
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Assistant/System used once routing layer wires multi-turn history
pub enum Role {
    User,
    Assistant,
    System,
}

/// Server-side / client-side tools the model may call.
#[derive(Debug, Clone)]
pub enum ToolSpec {
    /// Codex backend's built-in web_search (covers Search/OpenPage/FindInPage
    /// sub-actions; runs entirely on the OpenAI side, no client dispatch).
    WebSearch {
        /// `external_web_access=true` opts into live web access. Codex's
        /// "cached" mode would set this false (see codex-rs tool_spec.rs).
        external_web_access: bool,
    },
    /// Generic client-side function tool. The model emits `function_call`
    /// items; the agent loop dispatches via `tools::ToolRegistry`, then
    /// re-invokes `chat()` with `function_call_output` injected into
    /// `additional_inputs`.
    Function {
        name: String,
        description: String,
        /// JSONSchema describing the tool's argument object. Pass through
        /// to OpenAI Responses API verbatim.
        parameters: serde_json::Value,
    },
}

/// One of OpenAI's web_search sub-actions, mirrored from codex-rs protocol.
/// Surfaced to the UI so users see what the agent is searching / opening.
#[derive(Debug, Clone)]
pub enum WebSearchAction {
    Search {
        query: String,
        queries: Vec<String>,
        sources: Vec<String>,
    },
    OpenPage {
        url: String,
    },
    FindInPage {
        url: String,
        pattern: String,
    },
    Other,
}

#[derive(Debug, Clone)]
pub enum LlmEvent {
    TextDelta {
        text: String,
    },
    /// Lifecycle event for codex server-side `web_search`. Emitted twice per
    /// call: once on `output_item.added` (status=in_progress) and once on
    /// `output_item.done` (status=completed). The action carries the actual
    /// query / URL / pattern the model issued.
    WebSearchCall {
        status: String,
        action: Option<WebSearchAction>,
    },
    /// Client-side function tool invocation request from the model. The
    /// agent loop is expected to: (1) execute via `tools::ToolRegistry`,
    /// (2) inject the original `function_call` + corresponding
    /// `function_call_output` into `ChatRequest.additional_inputs`,
    /// (3) re-invoke `chat()` to let the model continue.
    FunctionCall {
        call_id: String,
        name: String,
        /// Already-complete arguments JSON (string form, as the codex
        /// backend ships it). Caller must `serde_json::from_str` to use.
        arguments: String,
    },
    /// Encrypted reasoning item from a reasoning model (gpt-5.x). The agent loop
    /// must round-trip it verbatim into the next request's input — placed before
    /// that iteration's `function_call` — so the codex backend's prompt cache
    /// keeps the chain-of-thought prefix across the tool loop. Dropping it
    /// breaks the cache on every post-reasoning tool step (OpenAI: round-tripping
    /// gives 40-80% better cache utilization). `summary` is the raw summary array
    /// the backend shipped; the item `id` is intentionally absent — codex omits
    /// it on replay since it references a server-side item that does not exist
    /// under `store:false`.
    Reasoning {
        encrypted_content: String,
        summary: serde_json::Value,
    },
    Usage(Usage),
    MessageEnd {
        stop_reason: StopReason,
    },
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // cache_write_tokens not surfaced by codex backend; wired for anthropic later
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    #[allow(dead_code)] // wired up when we add tool-use / errors in later slices
    Other,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    #[allow(dead_code)] // used by registry / fallback chain in later slices
    fn name(&self) -> &str;

    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>>;
}
