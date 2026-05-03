//! LLM provider abstraction.
//!
//! See `design/p1-spec/llm-provider.md` for the trait contract and the
//! three P1 providers. This module currently implements `codex_oauth` only;
//! `anthropic_api_key` and `openai_api_key` will land later.

pub mod codex_oauth;
pub mod openai_responses;

use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system: Option<String>,
    pub model: String,
    pub max_output_tokens: Option<u32>,
    pub tools: Vec<ToolSpec>,
    /// Raw input items appended after `messages` — used by the agent loop to
    /// inject prior-turn `function_call` and `function_call_output` items so
    /// the model has a complete view of the multi-turn tool dialog. Each item
    /// must already match the OpenAI Responses API input shape (see
    /// codex-rs/protocol/src/models.rs ResponseItem).
    pub additional_inputs: Vec<serde_json::Value>,
    /// Reasoning effort for models that support it (gpt-5/gpt-5.5/...).
    /// `None` means use the model's backend default. Compaction passes
    /// `Some(Minimal)` so summaries don't burn thinking tokens.
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// Mirrors codex-rs `ReasoningEffort`. We omit XHigh / None — leek only
/// needs the four levels users would actually pick.
#[derive(Debug, Clone, Copy)]
pub enum ReasoningEffort {
    Minimal,
    #[allow(dead_code)]
    Low,
    #[allow(dead_code)]
    Medium,
    #[allow(dead_code)]
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
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
    Search { query: String },
    OpenPage { url: String },
    FindInPage { url: String, pattern: String },
    Other,
}

#[derive(Debug, Clone)]
pub enum LlmEvent {
    TextDelta { text: String },
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
    Usage(Usage),
    MessageEnd { stop_reason: StopReason },
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

    async fn chat(
        &self,
        req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent>>>;
}
