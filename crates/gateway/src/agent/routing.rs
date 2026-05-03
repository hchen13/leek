//! Routing layer — decides whether a user message starts a new research task,
//! warrants a chat reply, or needs a clarifying question.
//!
//! See `design/p1-spec/agent-loop.md` §10 for the full protocol.
//! P1 simplification: this fires only when the session has no in-progress task.
//! When an in-progress task exists, messages attach to it directly.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;

use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role};

const ROUTING_MODEL: &str = "gpt-5.5";

const ROUTING_PROMPT: &str = r#"You are L.E.E.K's routing layer. Your only job: decide whether the latest user message starts a new research task or is just chat. Reply with strict JSON, no preamble, no markdown fences.

Schema:
{
  "decision": "new_task" | "chat_reply" | "ambiguous",
  "reason": "<short>",
  "task_draft": null | {
    "title": "<short>",
    "goal": "<1-2 sentences>",
    "expected_deliverable": "decision_draft" | "research_brief" | "review" | "comparison" | "morning_brief" | "free_form"
  },
  "chat_reply_text": null | "<full reply>",
  "clarification_question": null | "<one short question>"
}

Decision rules:
- new_task: user asks for research / decision / comparison / review on a specific subject, ticker, or company.
- chat_reply: greeting / casual / meta question about LEEK itself / one-shot definition that needs no investigation.
- ambiguous: subject unclear, needs one clarifying question.

expected_deliverable inference:
- buy/sell/add/trim/stop-loss => decision_draft
- post-mortem / look back at last decision => review
- compare / vs => comparison
- research / look into / 调研 / 研究 => research_brief
- morning brief / 晨报 => morning_brief
- otherwise => free_form

Examples:
- "你好" -> chat_reply, chat_reply_text greets back briefly
- "什么是经济护城河" -> chat_reply, chat_reply_text gives one-line definition (no investigation)
- "NVDA 现在能加仓吗？" -> new_task, decision_draft
- "看一下我的持仓" -> ambiguous (re-balance? risk check? performance review?)

Respond with the JSON object only.
"#;

const ROUTING_HISTORY_LIMIT: usize = 5;

#[derive(Debug, Clone)]
#[allow(dead_code)] // chat_reply_text not consumed — chat_reply path uses main pipeline for full history
pub struct RouteDecision {
    pub kind: DecisionKind,
    pub reason: String,
    pub task_draft: Option<TaskDraft>,
    pub chat_reply_text: Option<String>,
    pub clarification_question: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    NewTask,
    ChatReply,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct TaskDraft {
    pub title: String,
    pub goal: String,
    pub expected_deliverable: String,
}

pub async fn decide_route(
    provider: Arc<dyn LlmProvider>,
    history: &[ChatMessage],
    user_text: &str,
) -> Result<RouteDecision> {
    // Send the last few turns + the new user message. The new user message is
    // already the last item in `history` (POST handler persists before routing).
    let mut messages: Vec<ChatMessage> = history
        .iter()
        .rev()
        .take(ROUTING_HISTORY_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    messages.reverse();

    if messages
        .last()
        .map(|m| !matches!(m.role, Role::User) || m.content != user_text)
        .unwrap_or(true)
    {
        // Defensive: caller didn't include the new message in history yet.
        messages.push(ChatMessage {
            role: Role::User,
            content: user_text.to_string(),
        });
    }

    let req = ChatRequest {
        messages,
        system: Some(ROUTING_PROMPT.to_string()),
        model: ROUTING_MODEL.to_string(),
        max_output_tokens: None,
        // Routing is a deterministic intent classifier — no tool calls allowed.
        tools: Vec::new(),
        additional_inputs: Vec::new(),
        reasoning_effort: None,
    };

    let mut stream = provider.chat(req).await.context("routing chat call")?;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            LlmEvent::TextDelta { text: t } => text.push_str(&t),
            // Ignore usage / message_end for the routing pass — caller can
            // track aggregate usage separately if/when we surface it.
            _ => {}
        }
    }

    parse_decision(&text)
}

fn parse_decision(raw: &str) -> Result<RouteDecision> {
    let stripped = strip_code_fence(raw.trim());
    let v: serde_json::Value = serde_json::from_str(stripped).with_context(|| {
        format!(
            "routing LLM returned non-JSON (head): {}",
            stripped.chars().take(200).collect::<String>()
        )
    })?;

    let decision_str = v
        .get("decision")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("missing 'decision' field"))?;
    let kind = match decision_str {
        "new_task" => DecisionKind::NewTask,
        "chat_reply" => DecisionKind::ChatReply,
        "ambiguous" => DecisionKind::Ambiguous,
        other => return Err(anyhow!("unknown decision: {other}")),
    };
    let reason = v
        .get("reason")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let task_draft = v.get("task_draft").and_then(|td| {
        if td.is_null() {
            return None;
        }
        let obj = td.as_object()?;
        Some(TaskDraft {
            title: obj.get("title")?.as_str()?.to_string(),
            goal: obj.get("goal")?.as_str()?.to_string(),
            expected_deliverable: obj.get("expected_deliverable")?.as_str()?.to_string(),
        })
    });
    let chat_reply_text = v
        .get("chat_reply_text")
        .and_then(|s| s.as_str().map(|x| x.to_string()));
    let clarification_question = v
        .get("clarification_question")
        .and_then(|s| s.as_str().map(|x| x.to_string()));

    Ok(RouteDecision {
        kind,
        reason,
        task_draft,
        chat_reply_text,
        clarification_question,
    })
}

fn strip_code_fence(s: &str) -> &str {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```json").or_else(|| s.strip_prefix("```")) {
        let rest = rest.trim_start_matches('\n').trim();
        if let Some(stripped) = rest.strip_suffix("```") {
            return stripped.trim();
        }
        return rest;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_new_task() {
        let raw = r#"{"decision":"new_task","reason":"NVDA review","task_draft":{"title":"NVDA 加仓评估","goal":"...","expected_deliverable":"decision_draft"},"chat_reply_text":null,"clarification_question":null}"#;
        let d = parse_decision(raw).unwrap();
        assert_eq!(d.kind, DecisionKind::NewTask);
        let draft = d.task_draft.unwrap();
        assert_eq!(draft.expected_deliverable, "decision_draft");
        assert_eq!(draft.title, "NVDA 加仓评估");
    }

    #[test]
    fn parses_chat_reply() {
        let raw = r#"{"decision":"chat_reply","reason":"greeting","task_draft":null,"chat_reply_text":"你好！","clarification_question":null}"#;
        let d = parse_decision(raw).unwrap();
        assert_eq!(d.kind, DecisionKind::ChatReply);
        assert_eq!(d.chat_reply_text.unwrap(), "你好！");
    }

    #[test]
    fn parses_ambiguous() {
        let raw = r#"{"decision":"ambiguous","reason":"vague","task_draft":null,"chat_reply_text":null,"clarification_question":"哪只股票?"}"#;
        let d = parse_decision(raw).unwrap();
        assert_eq!(d.kind, DecisionKind::Ambiguous);
        assert_eq!(d.clarification_question.unwrap(), "哪只股票?");
    }

    #[test]
    fn strips_code_fence() {
        let raw = "```json\n{\"decision\":\"chat_reply\",\"reason\":\"\",\"task_draft\":null,\"chat_reply_text\":\"hi\",\"clarification_question\":null}\n```";
        let d = parse_decision(raw).unwrap();
        assert_eq!(d.kind, DecisionKind::ChatReply);
    }

    #[test]
    fn rejects_unknown_decision() {
        let raw = r#"{"decision":"maybe","task_draft":null,"chat_reply_text":null,"clarification_question":null}"#;
        assert!(parse_decision(raw).is_err());
    }
}
