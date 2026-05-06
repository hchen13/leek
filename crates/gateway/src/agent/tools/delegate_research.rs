use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agent::{harness, preview};
use crate::events::EventEnvelope;
use crate::llm::{
    ChatMessage, ChatRequest, LlmEvent, LlmProvider, ReasoningEffort, Role, ToolSpec,
};
use crate::vault::{events as vault_events, subagents as vault_subagents};

use super::{ToolContext, ToolHandler};

const TOOL_NAME: &str = "delegate_research";
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
                 Use this when the task needs a second specialized lens: data_scout, \
                 fundamental_analyst, trading_analyst, risk_manager, or corpus_guardian. \
                 Give the subagent enough context, retrieved facts, and corpus snippets; \
                 it cannot fetch data by itself in this slice."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "role": {
                        "type": "string",
                        "enum": [
                            "data_scout",
                            "fundamental_analyst",
                            "trading_analyst",
                            "risk_manager",
                            "corpus_guardian"
                        ],
                        "description": "Specialized subagent role"
                    },
                    "question": {
                        "type": "string",
                        "description": "Focused question for this subagent"
                    },
                    "context": {
                        "type": "string",
                        "description": "Relevant facts, quotes, tool outputs, corpus snippets, and user constraints"
                    },
                    "expected_output": {
                        "type": "string",
                        "description": "Optional shape of the answer, e.g. checklist, bear case, valuation bridge"
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
        let expected_output = args
            .get("expected_output")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("给出结论、证据、不确定性、反方或缺口。");

        let role_instruction = role_instruction(&role)?;
        let task_id = ctx.task_id.as_deref().unwrap_or("ad-hoc");
        let scope = serde_json::json!({
            "role": role,
            "expected_output": expected_output,
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
        let system = harness::build_subagent_prompt(&role, role_instruction);
        let user =
            format!("# 子任务\n{question}\n\n# 上下文\n{context}\n\n# 期望输出\n{expected_output}");
        let req = ChatRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: user,
            }],
            system: Some(system),
            model: SUBAGENT_MODEL.to_string(),
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
                        Ok(LlmEvent::WebSearchCall { .. } | LlmEvent::FunctionCall { .. }) => {}
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

fn required_str(args: &serde_json::Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("missing '{key}' argument"))
}

fn role_instruction(role: &str) -> Result<&'static str> {
    match role {
        "data_scout" => Ok(
            "你只负责事实盘点：已有数据说明了什么，缺了什么，哪些数据源还需要主 agent 继续查。不要给最终交易建议。",
        ),
        "fundamental_analyst" => Ok(
            "你负责商业质量、财务质量、估值和长期复利逻辑。必须区分可验证事实、估值假设和安全边际。",
        ),
        "trading_analyst" => Ok(
            "你负责短中期交易结构：催化、流动性、拥挤度、技术位置、入场/退出条件。必须指出失效条件。",
        ),
        "risk_manager" => Ok(
            "你负责反方和风控：找永久损失路径、流动性陷阱、仓位错误、叙事过热和用户 mandate 冲突。",
        ),
        "corpus_guardian" => Ok(
            "你负责检查分析是否偏离 corpus：是否忽略安全边际、能力圈、Mr. Market、反身性、债务周期或反方证据。",
        ),
        other => bail!("unknown subagent role: {other}"),
    }
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
