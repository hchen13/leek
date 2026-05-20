//! Auto-compaction — fold an over-long turn context into a traceable
//! summary so the turn can continue (REQUIREMENTS §7.1, MILESTONES M1.8).
//!
//! When a turn's context approaches the model's context window the loop
//! does not stop. It folds the *early* context — old session history,
//! early tool results, finished branches — into one summary block, keeps
//! the recent tail verbatim, and runs the next iteration with
//! `summary + recent tail`. The current user prompt, the most recent
//! messages and the most recent tool dialog are never folded.
//!
//! The summary is produced by one model call. Per REQUIREMENTS §7.1 there
//! is no "compaction failed → stop" guard: a failed summary call is an
//! ordinary model-call failure and surfaces through the loop's normal
//! fatal-error path.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures::StreamExt;

use crate::api::AppState;
use crate::llm::codex::CodexClient;
use crate::llm::{ChatMessage, ChatRequest, LlmEvent, Role};

/// Trailing session messages kept verbatim — never folded. Always includes
/// the current user prompt (the last message). Four ≈ the last two Q&A
/// exchanges: enough recent context for the model to continue without a
/// felt discontinuity, while everything older is covered by the summary.
const KEEP_MESSAGES: usize = 4;

/// Trailing function_call / function_call_output pairs kept verbatim. The
/// model needs its most recent tool round-trips intact to keep working;
/// earlier ones are folded into the summary.
const KEEP_DIALOG_PAIRS: usize = 3;

/// Prefixes the summary block when it is fed back into the model input.
const SUMMARY_HEADER: &str = "[已压缩的前文摘要] 以下是本回合早期上下文的可追溯摘要，\
原文已因接近上下文窗口上限而被压缩。请把摘要中的内容当作已经发生的事实，继续完成当前回合。";

/// Instructions for the summarization model call.
const COMPACTION_SYSTEM_PROMPT: &str = "\
你是 L.E.E.K agent 的上下文压缩器。下面给你的是一个**正在进行中**的 agent 回合的早期\
上下文（可能还包含上一次压缩生成的摘要）。这个回合尚未结束 —— 压缩完成后，agent 会\
带着你的摘要继续工作。

把这些早期上下文压缩成一份**可追溯的结构化摘要**。摘要会原样进入后续的模型输入、\
替换掉被压缩的原文，因此它必须让 agent 仅凭摘要就能无缝继续，不丢关键信息。

摘要必须完整保留：
1. 当前目标与用户约束 —— 用户要什么、有哪些明确边界和要求。
2. 已确认的事实与关键证据 —— 已查证为真的结论，连同支撑它的具体数据。
3. 工具调用结果摘要 —— 每个重要工具调用做了什么、得到什么，保留 provenance（工具名、\
关键参数、数据来源）。
4. 引用的来源 —— corpus 页面、网页 URL 等外部引用，逐条保留。
5. 未完成的分支与下一步意图 —— 尚未做完的子任务，以及 agent 接下来打算做什么。
6. 已触发的 guard 或错误状态 —— 例如时间软提示、工具失败等。

要求：
- 只做压缩，不推进任务，不下新结论，不调用工具。
- 宁可保留具体数字、名称、ID 和原文措辞，也不要泛化成“查了一些数据”。
- 已经无关的寒暄和重复内容可以丢弃。
- 用中文，结构化分点输出，不要任何开场白或结束语。";

/// Outcome of a compaction attempt.
#[derive(Debug)]
pub enum Compacted {
    /// The early context was folded into the summary. Carries an estimate
    /// of the post-compaction context size so the loop can reset its
    /// measured-token counter until the next iteration reports a real one.
    Done { tokens_after: u32 },
    /// Nothing old enough to fold — the loop proceeds with the iteration.
    /// This is also what keeps compaction from looping with no progress.
    Skipped,
}

/// The mutable turn context `compact` folds in place. Bundled into one handle
/// so `compact` carries a single parameter rather than three free-standing
/// `&mut`s — the running summary, the kept messages and the kept tool dialog
/// are one logical thing (the turn's evolving context).
pub struct TurnContext<'a> {
    /// Running summary of context folded on earlier passes. `compact`
    /// replaces it with a fresh summary that folds the previous one in.
    pub summary: &'a mut Option<String>,
    /// Session messages — drained of the early ones, left with the tail.
    pub messages: &'a mut Vec<ChatMessage>,
    /// Re-injected function_call / function_call_output dialog — likewise
    /// drained of the early pairs.
    pub tool_dialog: &'a mut Vec<serde_json::Value>,
}

/// Attempt one auto-compaction pass.
///
/// On `Done` the turn context (`ctx`) is mutated in place: the early messages
/// and tool dialog are dropped, `ctx.summary` is replaced with a fresh summary
/// (folding in any previous one), and `ctx.messages` / `ctx.tool_dialog` are
/// left holding only the recent tail. Emits `compaction_started` /
/// `compaction_completed`. An `Err` is a failed summary model call — the
/// caller treats it as an ordinary fatal model error.
/// `transcript_iter` is the iteration the compaction summary call should
/// occupy in the F2 `llm_transcripts` archive; the caller allocates it so
/// every LLM call inside a turn gets a unique row.
pub async fn compact(
    st: &AppState,
    session_id: &str,
    turn_id: &str,
    system: &str,
    ctx: TurnContext<'_>,
    tokens_before: u32,
    transcript_iter: i64,
) -> Result<Compacted> {
    let (early_msgs, early_items) = plan_split(ctx.messages.len(), ctx.tool_dialog.len());
    if early_msgs == 0 && early_items == 0 {
        return Ok(Compacted::Skipped);
    }

    st.emit(
        session_id,
        "compaction_started",
        serde_json::json!({ "turn_id": turn_id, "tokens_before": tokens_before }),
    )
    .await;

    let transcript = render_for_summary(
        ctx.summary.as_deref(),
        &ctx.messages[..early_msgs],
        &ctx.tool_dialog[..early_items],
    );
    let folded = summarize(
        &st.codex,
        st.guards.idle_timeout,
        transcript,
        session_id,
        turn_id,
        transcript_iter,
    )
    .await?;

    ctx.messages.drain(..early_msgs);
    ctx.tool_dialog.drain(..early_items);
    *ctx.summary = Some(folded);

    let tokens_after =
        estimate_context_tokens(system, ctx.summary.as_deref(), ctx.messages, ctx.tool_dialog);

    st.emit(
        session_id,
        "compaction_completed",
        serde_json::json!({
            "turn_id": turn_id,
            "tokens_before": tokens_before,
            "tokens_after": tokens_after,
            "messages_folded": early_msgs,
            "tool_calls_folded": early_items / 2,
            "summary_preview": super::preview(ctx.summary.as_deref().unwrap_or("")),
        }),
    )
    .await;

    tracing::info!(
        session_id,
        turn_id,
        tokens_before,
        tokens_after,
        messages_folded = early_msgs,
        "auto-compaction folded the turn context"
    );
    Ok(Compacted::Done { tokens_after })
}

/// Wrap the current summary as the developer-role context block prepended
/// to every post-compaction iteration's model input.
pub fn summary_message(summary: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Developer,
        content: format!("{SUMMARY_HEADER}\n\n{summary}"),
    }
}

/// Estimated token size of a fully assembled request: system prompt, the
/// compaction summary block (if present), session messages and tool dialog.
/// The loop uses this only before its first iteration reports a real count.
pub fn estimate_context_tokens(
    system: &str,
    summary: Option<&str>,
    messages: &[ChatMessage],
    tool_dialog: &[serde_json::Value],
) -> u32 {
    let mut total = estimate_tokens(system);
    if let Some(s) = summary {
        // The block is sent with its header prefix; count both.
        total = total
            .saturating_add(estimate_tokens(SUMMARY_HEADER))
            .saturating_add(estimate_tokens(s));
    }
    for m in messages {
        total = total.saturating_add(estimate_tokens(&m.content));
    }
    for item in tool_dialog {
        total = total.saturating_add(estimate_tokens(&item.to_string()));
    }
    total
}

/// Split point for compaction. Given the message count and tool-dialog item
/// count, returns `(early_messages, early_dialog_items)` — the counts to
/// fold away, keeping `KEEP_MESSAGES` trailing messages and
/// `KEEP_DIALOG_PAIRS` trailing call/output pairs verbatim.
fn plan_split(msg_len: usize, dialog_len: usize) -> (usize, usize) {
    let early_msgs = msg_len.saturating_sub(KEEP_MESSAGES);
    // tool_dialog is built strictly as function_call / function_call_output
    // pairs, so its length is even; keeping an even-length tail keeps whole
    // pairs (a kept function_call_output never loses its function_call).
    let keep_items = (KEEP_DIALOG_PAIRS * 2).min(dialog_len);
    let early_items = dialog_len - keep_items;
    (early_msgs, early_items)
}

/// Render the early context as a plain-text transcript for the summarizer.
/// Tool dialog is rendered as text rather than passed as structured items:
/// a summarization call carries no tools, and a bare `function_call_output`
/// would dangle without its `function_call`.
fn render_for_summary(
    prior: Option<&str>,
    messages: &[ChatMessage],
    tool_dialog: &[serde_json::Value],
) -> String {
    let mut t = String::new();

    if let Some(prior) = prior {
        t.push_str("## 此前已压缩的摘要\n\n");
        t.push_str(prior);
        t.push_str("\n\n");
    }

    if !messages.is_empty() {
        t.push_str("## 早期对话历史\n\n");
        for m in messages {
            let who = match m.role {
                Role::User => "用户",
                Role::Assistant => "助手",
                Role::Developer => "系统",
            };
            t.push_str(&format!("[{who}] {}\n\n", m.content));
        }
    }

    if !tool_dialog.is_empty() {
        t.push_str("## 早期工具调用\n\n");
        let mut n = 0;
        for item in tool_dialog {
            match item.get("type").and_then(|v| v.as_str()) {
                Some("function_call") => {
                    n += 1;
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let args = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                    t.push_str(&format!("### 工具调用 #{n} · {name}\n参数：{args}\n"));
                }
                Some("function_call_output") => {
                    let out = item.get("output").and_then(|v| v.as_str()).unwrap_or("");
                    t.push_str(&format!("结果：{out}\n\n"));
                }
                _ => {}
            }
        }
    }

    t
}

/// Run the summarization model call and collect its text. Uses a low
/// reasoning effort — summarization is not a deep-reasoning task and the
/// turn is under context (and possibly time) pressure. The call's own token
/// usage is intentionally not folded into the turn's token / cost totals:
/// compaction is tracked separately (compaction_count + compaction events).
async fn summarize(
    codex: &CodexClient,
    idle: Option<Duration>,
    transcript: String,
    session_id: &str,
    turn_id: &str,
    transcript_iter: i64,
) -> Result<String> {
    let req = ChatRequest {
        model: super::MODEL.to_string(),
        system: COMPACTION_SYSTEM_PROMPT.to_string(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: transcript,
        }],
        tools: Vec::new(),
        additional_inputs: Vec::new(),
        reasoning_effort: Some("low".to_string()),
        verbosity: None,
        // The summary call carries no tools — provider search included.
        web_search: false,
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        iteration: transcript_iter,
    };

    let mut stream = codex
        .chat(req)
        .await
        .context("auto-compaction: summary model call")?;

    let mut out = String::new();
    loop {
        // Idle-guard each chunk the same way the main loop does, so a hung
        // summary call fails instead of stalling the turn.
        let item = match idle {
            Some(d) => tokio::time::timeout(d, stream.next())
                .await
                .map_err(|_| anyhow::anyhow!("auto-compaction: summary call went idle"))?,
            None => stream.next().await,
        };
        let Some(event) = item else { break };
        if let LlmEvent::TextDelta { text } = event.context("auto-compaction: summary stream")? {
            out.push_str(&text);
        }
    }

    let out = out.trim().to_string();
    if out.is_empty() {
        bail!("auto-compaction produced an empty summary");
    }
    Ok(out)
}

/// Rough token estimate — about three UTF-8 bytes per token, a blended rate
/// for the mixed Chinese / English text leek handles. Only ever a *fallback*
/// signal: the provider's reported `input_tokens` is exact and supersedes it
/// on every iteration after the first.
fn estimate_tokens(s: &str) -> u32 {
    (s.len() / 3).try_into().unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.into(),
        }
    }

    #[test]
    fn plan_split_keeps_recent_message_tail() {
        assert_eq!(plan_split(2, 0), (0, 0));
        assert_eq!(plan_split(KEEP_MESSAGES, 0), (0, 0));
        assert_eq!(plan_split(10, 0), (10 - KEEP_MESSAGES, 0));
    }

    #[test]
    fn plan_split_folds_whole_dialog_pairs() {
        let keep = KEEP_DIALOG_PAIRS * 2;
        assert_eq!(plan_split(0, keep), (0, 0));
        // Folds an even count and keeps an even tail → no orphaned output.
        let (_, early) = plan_split(0, keep + 8);
        assert_eq!(early, 8);
        assert_eq!(early % 2, 0);
    }

    #[test]
    fn plan_split_skips_when_nothing_is_old() {
        assert_eq!(plan_split(KEEP_MESSAGES, KEEP_DIALOG_PAIRS * 2), (0, 0));
    }

    #[test]
    fn estimate_tokens_grows_with_length() {
        assert!(estimate_tokens("a fairly long-ish string") > estimate_tokens("hi"));
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_context_tokens_sums_its_parts() {
        let small = estimate_context_tokens("sys", None, &[], &[]);
        let big = estimate_context_tokens("sys", Some("a summary"), &[user("hello there")], &[]);
        assert!(big > small);
    }

    #[test]
    fn render_includes_prior_summary_messages_and_dialog() {
        let dialog = vec![
            serde_json::json!({
                "type": "function_call", "call_id": "c1",
                "name": "echo", "arguments": "{\"text\":\"x\"}"
            }),
            serde_json::json!({
                "type": "function_call_output", "call_id": "c1", "output": "echoed-x"
            }),
        ];
        let t = render_for_summary(Some("旧摘要内容"), &[user("早期问题")], &dialog);
        assert!(t.contains("旧摘要内容"));
        assert!(t.contains("早期问题"));
        assert!(t.contains("echo"));
        assert!(t.contains("结果：echoed-x"));
    }

    #[test]
    fn render_is_empty_for_empty_input() {
        assert!(render_for_summary(None, &[], &[]).is_empty());
    }

    #[test]
    fn summary_message_is_a_developer_block() {
        let m = summary_message("这是摘要正文");
        assert_eq!(m.role, Role::Developer);
        assert!(m.content.contains("这是摘要正文"));
        assert!(m.content.contains("可追溯摘要"));
    }
}
