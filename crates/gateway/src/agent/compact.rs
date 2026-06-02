//! Conversation compaction: summarize a session's history into a structured
//! markdown handoff so the same session can continue with far less context.
//!
//! Pipeline (P1, simple cut):
//!   1. Render messages → plain transcript text (role-prefixed)
//!   2. Send to LLM with structured-summary prompt + reasoning_effort=Low
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

/// Tool calls within the last N turns render with full detail; older ones
/// degrade to a `name(args)` index line (output dropped) so a long session
/// doesn't pay tokens for stale tool data verbatim. A new turn begins at each
/// `user` row. Override via `LEEK_TOOL_KEEP_TURNS`.
const TOOL_KEEP_TURNS_DEFAULT: usize = 6;

fn tool_keep_turns() -> usize {
    std::env::var("LEEK_TOOL_KEEP_TURNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(TOOL_KEEP_TURNS_DEFAULT)
}

/// Investment-research-flavored summary template. The agent must produce
/// these sections verbatim — leek frontend renders them as a system msg so
/// the user can audit what was kept vs dropped.
pub const COMPACT_SYSTEM_PROMPT: &str = "\
You are a conversation-summarization agent for **leek**, an investment-research \
agent system. Your job is to produce a structured *handoff summary* of a long \
session so the next turn can pick up with far less context.

CRITICAL RULES:
- Do NOT respond to any questions in the transcript.
- Do NOT take any actions, do not call any tools.
- Do NOT add information not present in the transcript.
- Durable tool output rows may be used as tool evidence.
- Web search activity/source hints only record search actions and possible
  sources; they do not contain fetched page content and must not be treated as
  evidence or confirmed facts.
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

## 用户偏好 / explicit constraint hints
<any signals about the user's risk tolerance, time horizon, constraints, or \
behavioral preferences that surfaced — only if explicit; do not invent>
";

/// Render the append-only conversation-queue tail into a plain transcript for
/// the summarizer. User/agent text renders verbatim; tool rows render inline so
/// the summary captures real tool evidence with no side channel. Tool calls
/// older than `tool_keep_turns()` turns drop their output and collapse to a
/// one-line `name(args)` index.
fn render_transcript(history: &[MessageRow]) -> String {
    // Assign a turn index to each row: a new turn starts at every user row.
    let mut turn_of = Vec::with_capacity(history.len());
    let mut turn = 0usize;
    for row in history {
        if row.role == "user" {
            turn += 1;
        }
        turn_of.push(turn);
    }
    let total_turns = turn;
    let keep = tool_keep_turns();

    let mut out = String::new();
    for (i, row) in history.iter().enumerate() {
        let recent = total_turns.saturating_sub(turn_of[i]) < keep;
        let content: serde_json::Value = match serde_json::from_str(&row.content_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match row.role.as_str() {
            "user" | "agent" => {
                let Some(text) = content.get("text").and_then(|t| t.as_str()) else {
                    continue;
                };
                let role_label = if row.role == "user" { "USER" } else { "LEEK" };
                out.push_str(&format!(
                    "--- [{role_label} · seq={} · {}]\n{}\n\n",
                    row.seq, row.created_at, text
                ));
            }
            "assistant_tool_calls" => {
                let Some(items) = content.get("items").and_then(|i| i.as_array()) else {
                    continue;
                };
                for it in items {
                    let name = it.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                    let args = it.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
                    if recent {
                        out.push_str(&format!(
                            "--- [TOOL CALL · {name}]\nargs={}\n\n",
                            preview(args, 400)
                        ));
                    } else {
                        out.push_str(&format!(
                            "--- [earlier tool: {name}({})]\n\n",
                            preview(args, 200)
                        ));
                    }
                }
            }
            "tool_result" => {
                // Older tool output is dropped; the name(args) index is enough.
                if !recent {
                    continue;
                }
                let output = content.get("output").and_then(|o| o.as_str()).unwrap_or("");
                out.push_str(&format!("--- [TOOL RESULT]\n{}\n\n", preview(output, 1200)));
            }
            _ => continue,
        }
    }
    out
}

fn preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out = s.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

/// Run the structured-summary call against the LLM provider. Returns the
/// markdown blob (the agent's full reply); caller decides what to do with it
/// (persist, rebroadcast over SSE, inject into the same session, etc.).
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
        session_id: None,
        prompt_cache_key: Some(super::prompt_cache_key_for(
            COMPACT_MODEL,
            "global",
            "compact",
        )),
        max_output_tokens: None,
        tools: Vec::new(),
        additional_inputs: Vec::new(),
        // gpt-5.5 rejects `minimal` (400: "Unsupported value … Supported
        // values are: none, low, medium, high, xhigh"). `low` is the lightest
        // level it accepts and is enough for a structured summary without
        // burning thinking tokens.
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

    fn mk_tool_calls(seq: i64, items: serde_json::Value) -> MessageRow {
        MessageRow {
            seq,
            task_id: None,
            role: "assistant_tool_calls".into(),
            content_json: serde_json::json!({ "type": "tool_calls", "items": items }).to_string(),
            created_at: "2026-05-03T00:00:00Z".into(),
        }
    }

    fn mk_tool_result(seq: i64, call_id: &str, output: &str) -> MessageRow {
        MessageRow {
            seq,
            task_id: None,
            role: "tool_result".into(),
            content_json: serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            })
            .to_string(),
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

    #[test]
    fn render_transcript_renders_tool_rows_inline() {
        let rows = vec![
            mk_row(1, "user", "查一下 NVDA"),
            mk_tool_calls(
                2,
                serde_json::json!([
                    {"type":"function_call","call_id":"c1","name":"web_fetch","arguments":"{\"url\":\"https://x\"}"}
                ]),
            ),
            mk_tool_result(3, "c1", "NVIDIA reported data center revenue growth."),
            mk_row(4, "agent", "数据中心营收高增。"),
        ];
        let t = render_transcript(&rows);
        assert!(t.contains("TOOL CALL · web_fetch"));
        assert!(t.contains("TOOL RESULT"));
        assert!(t.contains("data center revenue"));
    }

    #[test]
    fn render_transcript_degrades_old_tool_calls_to_name_args() {
        std::env::set_var("LEEK_TOOL_KEEP_TURNS", "1");
        // Turn 1's tool output should be dropped; only its name(args) index kept.
        let rows = vec![
            mk_row(1, "user", "第一问"),
            mk_tool_calls(
                2,
                serde_json::json!([
                    {"type":"function_call","call_id":"c1","name":"get_financials","arguments":"{\"ts_code\":\"600519\"}"}
                ]),
            ),
            mk_tool_result(3, "c1", "OLD_OUTPUT_SHOULD_BE_DROPPED 营收 1700 亿"),
            mk_row(4, "user", "第二问"),
            mk_row(5, "agent", "答案。"),
        ];
        let t = render_transcript(&rows);
        std::env::remove_var("LEEK_TOOL_KEEP_TURNS");
        assert!(t.contains("earlier tool: get_financials("));
        assert!(t.contains("600519"));
        assert!(!t.contains("OLD_OUTPUT_SHOULD_BE_DROPPED"));
    }
}
