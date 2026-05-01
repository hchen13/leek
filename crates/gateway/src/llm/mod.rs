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

#[derive(Debug, Clone)]
pub enum LlmEvent {
    TextDelta { text: String },
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
