//! Conversation compaction: summarize a session's history into a structured
//! markdown handoff so a forked session can pick up with far less context.
//!
//! Pipeline (P1, simple cut):
//!   1. Render messages → plain transcript text (role-prefixed)
//!   2. Send to LLM with structured-summary prompt + reasoning_effort=Minimal
//!   3. Collect text deltas, return as one Markdown blob
//!
//! Future iterations (not yet wired):
//!   - Tool output pruning (replace large tool results with one-line stubs)
//!   - Tail preservation (keep last N tokens verbatim, summarize only head)
//!   - Iterative `_previous_summary` for sessions compacted > once

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, ReasoningEffort, Role};
use crate::vault::messages::MessageRow;

const COMPACT_MODEL: &str = "gpt-5.5";

/// Investment-research-flavored summary template. The agent must produce
/// these sections verbatim — leek frontend renders them as a system msg so
/// the user can audit what was kept vs dropped.
pub const COMPACT_SYSTEM_PROMPT: &str = "\
You are a conversation-summarization agent for **leek**, an investment-research \
agent system. Your job is to produce a structured *handoff summary* of a long \
session so a forked session can pick up with far less context.

CRITICAL RULES:
- Do NOT respond to any questions in the transcript.
- Do NOT take any actions, do not call any tools.
- Do NOT add information not present in the transcript.
- Output Chinese (zh-CN) — match the language the user has been using.

Output EXACTLY this Markdown structure, in order:

## 当前研究主题
<one or two sentences identifying what the user has been asking about; if \
multiple threads, list each as a sub-bullet>

## 已确认事实
<bullet list of facts established in the conversation, with numbers / dates / \
sources where they appear in the transcript>

## 已查 corpus / 已用工具
<which corpus pages were searched, which tools were called, salient outputs>

## 仍未回答 / 待跟进
<questions the user asked but the agent didn't fully resolve, or follow-ups \
the agent committed to>

## 用户偏好 / mandate hints
<any signals about the user's risk tolerance, time horizon, mandate, or \
behavioral preferences that surfaced — only if explicit; do not invent>
";

/// Render a vault message list into a plain transcript suitable for LLM
/// consumption. user/agent text rows only — tool rows live in the events
/// table and aren't surfaced in P1.
fn render_transcript(history: &[MessageRow]) -> String {
    let mut out = String::new();
    for row in history {
        let content: serde_json::Value = match serde_json::from_str(&row.content_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let text = match content.get("text").and_then(|t| t.as_str()) {
            Some(t) => t,
            None => continue,
        };
        let role_label = match row.role.as_str() {
            "user" => "USER",
            "agent" => "LEEK",
            _ => continue,
        };
        out.push_str(&format!(
            "--- [{role_label} · seq={} · {}]\n{}\n\n",
            row.seq, row.created_at, text
        ));
    }
    out
}

/// Run the structured-summary call against the LLM provider. Returns the
/// markdown blob (the agent's full reply); caller decides what to do with it
/// (persist, rebroadcast over SSE, fork a new session, etc.).
///
/// `focus`, when supplied, is appended to the user message so the summarizer
/// pays extra attention to that thread; otherwise compaction is free-form.
///
/// `cancel` is honored between stream events — manual /compact via UI sets
/// this when the user hits Esc; auto-compaction passes a never-firing token.
pub async fn summarize_session(
    provider: Arc<dyn LlmProvider>,
    history: &[MessageRow],
    focus: Option<&str>,
    cancel: CancellationToken,
) -> Result<String> {
    if history.is_empty() {
        return Err(anyhow!("compact: refusing to summarize empty history"));
    }

    let transcript = render_transcript(history);
    let mut user_msg = format!(
        "Below is the transcript from one leek session. Produce the structured \
         summary as instructed.\n\n{transcript}"
    );
    if let Some(f) = focus {
        user_msg.push_str(&format!(
            "\n--- FOCUS ---\nPay special attention to: {f}\nCover other \
             threads in single-sentence form only.\n"
        ));
    }

    let req = ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: user_msg,
        }],
        system: Some(COMPACT_SYSTEM_PROMPT.to_string()),
        model: COMPACT_MODEL.to_string(),
        max_output_tokens: None,
        tools: Vec::new(),
        additional_inputs: Vec::new(),
        // gpt-5.5 doesn't accept `minimal`; `low` is the lightest level it
        // supports and is enough for a structured summary without burning
        // thinking tokens.
        reasoning_effort: Some(ReasoningEffort::Low),
    };

    let mut stream = provider.chat(req).await.context("compact: chat call")?;
    let mut summary = String::new();
    while let Some(evt) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(anyhow!("compact: cancelled"));
        }
        let evt = evt.context("compact: stream event")?;
        match evt {
            LlmEvent::TextDelta { text } => summary.push_str(&text),
            LlmEvent::MessageEnd { .. } => break,
            _ => {}
        }
    }

    if summary.trim().is_empty() {
        return Err(anyhow!("compact: LLM returned empty summary"));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_row(seq: i64, role: &str, text: &str) -> MessageRow {
        MessageRow {
            seq,
            task_id: None,
            role: role.into(),
            content_json: serde_json::json!({ "text": text }).to_string(),
            created_at: "2026-05-03T00:00:00Z".into(),
        }
    }

    #[test]
    fn render_transcript_includes_user_and_agent_rows() {
        let rows = vec![
            mk_row(1, "user", "贵州茅台最近怎么样？"),
            mk_row(2, "agent", "近 60 个交易日宽幅震荡，回到 1400 附近。"),
            mk_row(3, "user", "估值合理吗？"),
        ];
        let t = render_transcript(&rows);
        assert!(t.contains("USER"));
        assert!(t.contains("LEEK"));
        assert!(t.contains("贵州茅台"));
        assert!(t.contains("1400"));
        assert!(t.contains("seq=2"));
    }

    #[test]
    fn render_transcript_skips_unknown_roles() {
        let rows = vec![
            mk_row(1, "user", "hi"),
            mk_row(2, "system", "diagnostic"),
            mk_row(3, "agent", "hello"),
        ];
        let t = render_transcript(&rows);
        assert!(!t.contains("diagnostic"));
        assert!(t.contains("hi"));
        assert!(t.contains("hello"));
    }

    #[test]
    fn render_transcript_skips_malformed_content_json() {
        let mut bad = mk_row(1, "user", "ok");
        bad.content_json = "not-json".into();
        let good = mk_row(2, "user", "good");
        let t = render_transcript(&[bad, good]);
        assert!(!t.contains("not-json"));
        assert!(t.contains("good"));
    }
}
