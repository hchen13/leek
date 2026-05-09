use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::{ToolContext, ToolHandler};

pub const TOOL_NAME: &str = "ask_user_question";

pub struct AskUserQuestionTool;

impl AskUserQuestionTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for AskUserQuestionTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Ask the user one to three short clarifying questions and pause the current task. \
                Use this only when the answer materially changes the scope, risk tolerance, time horizon, \
                or expected depth, and the information cannot be safely inferred or discovered with read-only tools."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 3,
                        "description": "Questions to show the user. Prefer one question.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {
                                    "type": "string",
                                    "description": "Stable snake_case identifier for mapping the answer"
                                },
                                "header": {
                                    "type": "string",
                                    "description": "Short UI label, 12 characters or fewer"
                                },
                                "question": {
                                    "type": "string",
                                    "description": "Single-sentence prompt shown to the user"
                                },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 3,
                                    "description": "Mutually exclusive choices. Put the recommended option first and suffix its label with (Recommended).",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "User-facing label"
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "One short sentence explaining impact or tradeoff"
                                            }
                                        },
                                        "required": ["label", "description"]
                                    }
                                }
                            },
                            "required": ["id", "header", "question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        _ctx: &ToolContext,
    ) -> Result<String> {
        let questions = args
            .get("questions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("missing 'questions' array"))?;
        if questions.is_empty() || questions.len() > 3 {
            return Err(anyhow!("ask_user_question requires one to three questions"));
        }
        for question in questions {
            required_str(question, "id")?;
            required_str(question, "header")?;
            required_str(question, "question")?;
            let options = question
                .get("options")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("each question requires options"))?;
            if !(2..=3).contains(&options.len()) {
                return Err(anyhow!("each question requires two or three options"));
            }
            for option in options {
                required_str(option, "label")?;
                required_str(option, "description")?;
            }
        }

        let question_text = questions
            .iter()
            .filter_map(|q| q.get("question").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(serde_json::json!({
            "status": "awaiting_user",
            "question_text": question_text,
            "questions": questions,
        })
        .to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventBus;
    use crate::vault;

    async fn ctx() -> ToolContext {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ToolContext {
            pool,
            event_bus: EventBus::new(),
            user_id: vault::LOCAL_USER_ID.to_string(),
            session_id: "s".to_string(),
            task_id: Some("t".to_string()),
            tuning: crate::llm::LlmTuning::defaults(),
        }
    }

    #[tokio::test]
    async fn returns_awaiting_user_payload() {
        let tool = AskUserQuestionTool::new();
        let out = tool
            .call(
                serde_json::json!({
                    "questions": [{
                        "id": "time_horizon",
                        "header": "周期",
                        "question": "这次研究按短线还是中长期来做？",
                        "options": [
                            {"label": "中长期 (Recommended)", "description": "更适合完整基本面研究。"},
                            {"label": "短线", "description": "更强调催化、资金面和退出。"}
                        ]
                    }]
                }),
                CancellationToken::new(),
                &ctx().await,
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["status"], "awaiting_user");
        assert!(value["question_text"].as_str().unwrap().contains("短线"));
    }

    #[tokio::test]
    async fn rejects_missing_options() {
        let tool = AskUserQuestionTool::new();
        let err = tool
            .call(
                serde_json::json!({
                    "questions": [{
                        "id": "scope",
                        "header": "范围",
                        "question": "要做到什么程度？"
                    }]
                }),
                CancellationToken::new(),
                &ctx().await,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("options"));
    }
}
