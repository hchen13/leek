//! Client-side tool dispatch.
//!
//! The agent loop registers tools here, then advertises them to the LLM via
//! `ChatRequest.tools`. When the model issues a `function_call` event the
//! loop calls `ToolRegistry::dispatch`, captures the result, and re-feeds it
//! into the next turn via `ChatRequest.additional_inputs`.
//!
//! Server-side tools (codex's built-in `web_search`) bypass this — they're
//! advertised through `ToolSpec::WebSearch` and execute on OpenAI's side.

pub mod corpus_search;
pub mod sec_filing_fetch;
pub mod tradingview_quote;
pub mod tushare_quote;
pub mod web_fetch;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    /// Advertise the tool to the LLM. Must return `ToolSpec::Function { ... }`.
    fn spec(&self) -> ToolSpec;
    /// Execute the tool. `args` is already parsed from the wire string.
    /// `cancel` fires when the user aborts the agent — handlers should
    /// treat it as a hard stop and return promptly.
    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<String>;
}

#[derive(Default, Clone)]
pub struct ToolRegistry {
    handlers: Arc<HashMap<String, Arc<dyn ToolHandler>>>,
}

impl ToolRegistry {
    pub fn builder() -> ToolRegistryBuilder {
        ToolRegistryBuilder::default()
    }

    pub fn empty() -> Self {
        Self::default()
    }

    /// All advertised tool specs (passed verbatim to `ChatRequest.tools`).
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.handlers.values().map(|h| h.spec()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Look up + invoke a tool by name. Errors propagate as `Err`; handler-
    /// level errors are the handler's job to format (we do NOT swallow them
    /// here — caller decides whether to feed them back to the model).
    pub async fn dispatch(
        &self,
        name: &str,
        raw_args: &str,
        cancel: CancellationToken,
    ) -> Result<String> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| anyhow!("unknown tool: {name}"))?;
        let args: serde_json::Value = if raw_args.trim().is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(raw_args)
                .map_err(|e| anyhow!("invalid arguments JSON for {name}: {e}"))?
        };
        handler.call(args, cancel).await
    }
}

#[derive(Default)]
pub struct ToolRegistryBuilder {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistryBuilder {
    pub fn register(mut self, handler: Arc<dyn ToolHandler>) -> Self {
        self.handlers.insert(handler.name().to_string(), handler);
        self
    }

    pub fn build(self) -> ToolRegistry {
        ToolRegistry {
            handlers: Arc::new(self.handlers),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoTool;

    #[async_trait]
    impl ToolHandler for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn spec(&self) -> ToolSpec {
            ToolSpec::Function {
                name: "echo".into(),
                description: "Returns its input unchanged.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                }),
            }
        }
        async fn call(
            &self,
            args: serde_json::Value,
            _cancel: CancellationToken,
        ) -> Result<String> {
            Ok(args
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string())
        }
    }

    #[tokio::test]
    async fn dispatch_invokes_handler() {
        let reg = ToolRegistry::builder()
            .register(Arc::new(EchoTool))
            .build();
        let out = reg
            .dispatch("echo", r#"{"text":"hi"}"#, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out, "hi");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_errors() {
        let reg = ToolRegistry::empty();
        let err = reg
            .dispatch("nope", "{}", CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_invalid_json_args_errors() {
        let reg = ToolRegistry::builder()
            .register(Arc::new(EchoTool))
            .build();
        let err = reg
            .dispatch("echo", "not json", CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid arguments JSON"));
    }

    #[tokio::test]
    async fn empty_args_treated_as_empty_object() {
        let reg = ToolRegistry::builder()
            .register(Arc::new(EchoTool))
            .build();
        let out = reg.dispatch("echo", "", CancellationToken::new()).await.unwrap();
        // EchoTool returns "" when text key absent — that's the contract.
        assert_eq!(out, "");
    }

    #[test]
    fn specs_round_trips_function_tool() {
        let reg = ToolRegistry::builder()
            .register(Arc::new(EchoTool))
            .build();
        let specs = reg.specs();
        assert_eq!(specs.len(), 1);
        match &specs[0] {
            ToolSpec::Function { name, .. } => assert_eq!(name, "echo"),
            other => panic!("expected Function, got {other:?}"),
        }
    }
}
