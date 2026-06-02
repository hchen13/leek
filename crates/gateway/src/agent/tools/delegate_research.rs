use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agent::{harness, preview, prompt_cache_key_for};
use crate::events::EventEnvelope;
use crate::llm::{
    ChatMessage, ChatRequest, LlmEvent, LlmProvider, ReasoningEffort, Role, ToolSpec,
};
use crate::vault::{events as vault_events, subagents as vault_subagents};

use super::{ToolContext, ToolHandler};

pub(crate) const TOOL_NAME: &str = "delegate_research";
const SUBAGENT_MODEL: &str = "gpt-5.5";

pub struct DelegateResearchTool {
    provider: Arc<dyn LlmProvider>,
}

impl DelegateResearchTool {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl ToolHandler for DelegateResearchTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Run a focused financial-research subagent and return its independent report. \
                 Use this when the task needs a separate worker with a role, thinking frame, \
                 and output shape defined by the main agent. \
                 Give the subagent enough context, retrieved facts, and corpus snippets; \
                 it cannot fetch data by itself in this slice. Reuse prior tool evidence in \
                 the context instead of asking the subagent to rediscover facts."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "role": {
                        "type": "string",
                        "description": "Role or persona for this subagent, defined for this specific task"
                    },
                    "question": {
                        "type": "string",
                        "description": "Focused question for this subagent"
                    },
                    "context": {
                        "type": "string",
                        "description": "Relevant facts, quotes, tool outputs, corpus snippets, and user constraints"
                    },
                    "thinking_frame": {
                        "type": "string",
                        "description": "Optional method or lens this worker should use"
                    },
                    "output_shape": {
                        "type": "string",
                        "description": "Optional requested form for the answer"
                    }
                },
                "required": ["role", "question", "context"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        cancel: CancellationToken,
        ctx: &ToolContext,
    ) -> Result<String> {
        let role = required_str(&args, "role")?;
        let question = required_str(&args, "question")?;
        let context = required_str(&args, "context")?;
        let thinking_frame = args
            .get("thinking_frame")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("");
        let output_shape = args
            .get("output_shape")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("");

        let task_id = ctx.task_id.as_deref().unwrap_or("ad-hoc");
        let scope = serde_json::json!({
            "role": role,
            "thinking_frame": thinking_frame,
            "output_shape": output_shape,
        });
        let input = serde_json::json!({
            "question": question,
            "context": context,
        });
        let run_id = vault_subagents::start(
            &ctx.pool,
            &ctx.user_id,
            vault_subagents::SubagentStart {
                session_id: &ctx.session_id,
                task_id,
                spec_name: &role,
                scope_json: &scope,
                input_json: &input,
            },
        )
        .await?;

        publish_subagent_event(
            ctx,
            &run_id,
            "in_progress",
            &role,
            &question,
            serde_json::Value::Null,
        )
        .await;

        let started = Instant::now();
        let role_instruction = format_subagent_instruction(&role, thinking_frame, output_shape);
        let system = harness::build_subagent_prompt(&role, &role_instruction);
        let mut user = format!("# 子任务\n{question}\n\n# 上下文\n{context}");
        if !thinking_frame.is_empty() {
            user.push_str(&format!("\n\n# 思维框架\n{thinking_frame}"));
        }
        if !output_shape.is_empty() {
            user.push_str(&format!("\n\n# 输出要求\n{output_shape}"));
        }
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: user,
            }],
            system: Some(system),
            model: SUBAGENT_MODEL.to_string(),
            session_id: Some(ctx.session_id.clone()),
            prompt_cache_key: Some(prompt_cache_key_for(
                SUBAGENT_MODEL,
                &ctx.session_id,
                &format!("subagent:{run_id}"),
            )),
            max_output_tokens: Some(2400),
            tools: Vec::new(),
            additional_inputs: Vec::new(),
            reasoning_effort: Some(ReasoningEffort::High),
        };

        let mut output = String::new();
        let mut tokens_used = 0i64;
        let mut stream = match self.provider.chat(req).await {
            Ok(stream) => stream,
            Err(err) => {
                finish_failed(ctx, &run_id, started, &err.to_string()).await?;
                return Err(err);
            }
        };

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    finish_failed(ctx, &run_id, started, "subagent cancelled").await?;
                    bail!("subagent cancelled");
                }
                evt = stream.next() => {
                    let Some(evt) = evt else { break };
                    match evt {
                        Ok(LlmEvent::TextDelta { text }) => output.push_str(&text),
                        Ok(LlmEvent::Usage(u)) => {
                            tokens_used += i64::from(u.input_tokens) + i64::from(u.output_tokens);
                        }
                        Ok(LlmEvent::MessageEnd { .. }) => {}
                        Ok(
                            LlmEvent::WebSearchCall { .. }
                            | LlmEvent::FunctionCall { .. }
                            | LlmEvent::Reasoning { .. },
                        ) => {}
                        Err(err) => {
                            finish_failed(ctx, &run_id, started, &err.to_string()).await?;
                            return Err(err);
                        }
                    }
                }
            }
        }

        let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
        let output_json = serde_json::json!({
            "role": role,
            "question": question,
            "output": output,
        });
        vault_subagents::finish(
            &ctx.pool,
            &ctx.user_id,
            &run_id,
            Some(&output_json),
            true,
            None,
            tokens_used,
            1,
            duration_ms,
        )
        .await?;
        publish_subagent_event(
            ctx,
            &run_id,
            "completed",
            &role,
            &question,
            serde_json::json!({
                "output_preview": preview(&output, 1200),
                "output_bytes": output.len(),
                "duration_ms": duration_ms,
            }),
        )
        .await;

        Ok(format!(
            "## Subagent `{role}` result\n\nrun_id: `{run_id}`\n\n{output}"
        ))
    }
}

fn format_subagent_instruction(role: &str, thinking_frame: &str, output_shape: &str) -> String {
    let mut instruction = format!("你的角色由主 agent 为本次任务定义为：{role}。");
    if !thinking_frame.is_empty() {
        instruction.push_str(&format!(" 使用这个思维框架：{thinking_frame}。"));
    }
    if !output_shape.is_empty() {
        instruction.push_str(&format!(" 输出形态按这个要求组织：{output_shape}。"));
    }
    instruction
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::BoxStream;

    struct NullProvider;

    #[async_trait]
    impl LlmProvider for NullProvider {
        fn name(&self) -> &str {
            "null"
        }

        async fn chat(&self, _req: ChatRequest) -> Result<BoxStream<'static, Result<LlmEvent>>> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    #[test]
    fn spec_does_not_force_known_roles_or_output_aliases() {
        let provider = Arc::new(NullProvider);
        let tool = DelegateResearchTool::new(provider);
        let ToolSpec::Function { parameters, .. } = tool.spec() else {
            panic!("expected function spec");
        };
        let role = &parameters["properties"]["role"];
        assert!(role.get("enum").is_none());
        assert!(parameters["properties"].get("expected_output").is_none());
        assert!(parameters["properties"].get("output_shape").is_some());
    }

    #[test]
    fn subagent_instruction_is_defined_by_call_arguments() {
        let text =
            format_subagent_instruction("bear-case reviewer", "只检查永久损失路径", "列三条证据");
        assert!(text.contains("bear-case reviewer"));
        assert!(text.contains("只检查永久损失路径"));
        assert!(text.contains("列三条证据"));
    }
}

fn required_str(args: &serde_json::Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing '{key}' argument"))
}

async fn finish_failed(
    ctx: &ToolContext,
    run_id: &str,
    started: Instant,
    error: &str,
) -> Result<()> {
    let duration_ms = started.elapsed().as_millis().try_into().unwrap_or(i64::MAX);
    vault_subagents::finish(
        &ctx.pool,
        &ctx.user_id,
        run_id,
        None,
        false,
        Some(error),
        0,
        0,
        duration_ms,
    )
    .await?;
    publish_subagent_event(
        ctx,
        run_id,
        "error",
        "",
        "",
        serde_json::json!({ "error": error, "duration_ms": duration_ms }),
    )
    .await;
    Ok(())
}

async fn publish_subagent_event(
    ctx: &ToolContext,
    run_id: &str,
    status: &str,
    role: &str,
    question: &str,
    extra: serde_json::Value,
) {
    let payload = serde_json::json!({
        "run_id": run_id,
        "status": status,
        "role": role,
        "question": question,
        "extra": extra,
    });
    let ts = chrono::Utc::now();
    let evt_seq = vault_events::insert(
        &ctx.pool,
        &ctx.user_id,
        &ctx.session_id,
        ctx.task_id.as_deref(),
        "subagent_run",
        &payload,
        Some("subagent"),
        ts,
    )
    .await
    .unwrap_or(0);
    ctx.event_bus
        .publish(
            &ctx.session_id,
            EventEnvelope {
                seq: evt_seq,
                kind: "subagent_run".to_string(),
                payload,
                ts,
            },
        )
        .await;
}
