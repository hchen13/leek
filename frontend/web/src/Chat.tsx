// Chat — the user's input and final-reply column (REQUIREMENTS §2.1).
//
// Chat holds only user messages, assistant final replies, and a compact
// tool / progress summary per turn. Note Trace never appears here — it is
// canvas content. While a turn runs the summary lists each tool / search
// call live; once the turn ends it folds to one aggregate line that
// expands again on click. Clicking a summary item focuses its canvas card.

import { createSignal, For, Show } from "solid-js";

import { renderMarkdown } from "./markdown";
import type { Message, Turn } from "./types";

type Streaming = { turnId: string; iteration: number; text: string };

type ChatProps = {
  messages: () => Message[];
  turns: () => Turn[];
  streaming: () => Streaming | null;
  noted: () => Record<string, true>;
  sending: () => boolean;
  send: (text: string) => void;
  focusCard: (artifactId: string, isError: boolean) => void;
};

type SummaryStatus = "running" | "ok" | "error";
type SummaryItem = {
  artifactId: string;
  label: string;
  detail: string;
  status: SummaryStatus;
};

/** One summary line per tool / search call of a turn — notes are excluded
 *  (notes are canvas-only). */
function summaryItems(turn: Turn): SummaryItem[] {
  return turn.artifacts
    .filter((a) => a.kind !== "note")
    .map((a) => ({
      artifactId: a.artifactId,
      label: a.kind === "search" ? "网页搜索" : a.displayName ?? a.tool ?? "工具",
      detail: a.kind === "search" ? a.query ?? "" : a.summary ?? "",
      status: a.phase === "start" ? "running" : a.phase === "error" ? "error" : "ok",
    }));
}

function statusIcon(s: SummaryStatus): string {
  return s === "ok" ? "✓" : s === "error" ? "✗" : "▸";
}

function aggregateText(items: SummaryItem[]): string {
  const fails = items.filter((i) => i.status === "error").length;
  const cards = items.filter((i) => i.status === "ok").length;
  let text = `已执行 ${items.length} 步`;
  if (fails > 0) text += ` · ${fails} 个失败`;
  if (cards > 0) text += ` · ${cards} 个数据卡片`;
  return text;
}

export function Chat(props: ChatProps) {
  const [draft, setDraft] = createSignal("");
  const [expanded, setExpanded] = createSignal<Set<string>>(new Set());

  const toggleSummary = (turnId: string) => {
    const next = new Set(expanded());
    if (next.has(turnId)) next.delete(turnId);
    else next.add(turnId);
    setExpanded(next);
  };

  const runningTurn = () => props.turns().find((t) => t.status === "running");
  const turnForMessage = (seq: number) => props.turns().find((t) => t.messageSeq === seq);

  const submit = () => {
    const text = draft().trim();
    if (!text) return;
    setDraft("");
    props.send(text);
  };

  // The optimistic streaming bubble — shown only while the current
  // iteration's text is the final reply. A `note_trace` for that iteration
  // marks the text as narration, and the bubble drops it (REQUIREMENTS §2.3).
  const bubbleText = (): string => {
    const s = props.streaming();
    if (!s) return "";
    if (props.noted()[`${s.turnId}:${s.iteration}`]) return "";
    return s.text;
  };

  const renderSummary = (turn: Turn | undefined) => {
    if (!turn) return null;
    const items = summaryItems(turn);
    if (items.length === 0) return null;
    const running = turn.status === "running";
    const open = () => running || expanded().has(turn.turnId);
    return (
      <div class="tool-summary">
        <Show when={!running}>
          <button class="summary-aggregate" onClick={() => toggleSummary(turn.turnId)}>
            <span class="summary-caret">{open() ? "▾" : "▸"}</span>
            {aggregateText(items)}
          </button>
        </Show>
        <Show when={open()}>
          <ul class="summary-list">
            <For each={items}>
              {(it) => (
                <li
                  classList={{ "summary-item": true, [`s-${it.status}`]: true }}
                  onClick={() => props.focusCard(it.artifactId, it.status === "error")}
                  title="跳转到 canvas 卡片"
                >
                  <span class="summary-icon">{statusIcon(it.status)}</span>
                  <span class="summary-label">{it.label}</span>
                  <Show when={it.detail}>
                    <span class="summary-detail">{it.detail}</span>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </div>
    );
  };

  return (
    <section class="chat">
      <header class="panel-head">
        <h2>Chat</h2>
      </header>
      <div class="messages">
        <For
          each={props.messages()}
          fallback={<p class="muted">还没有消息。在下方输入一条研究问题。</p>}
        >
          {(m) => (
            <>
              <Show when={m.role === "assistant"}>{renderSummary(turnForMessage(m.seq))}</Show>
              <div classList={{ msg: true, [m.role]: true }}>
                <span class="role">{m.role}</span>
                {/* User and assistant bodies both go through markdown so a
                    user-pasted snippet renders the same as an assistant
                    reply. innerHTML is reactive — Solid's JSX compiler
                    wraps the RHS in an effect that re-evaluates when
                    `m` (or any signal it reads) changes. */}
                <div class="msg-body markdown-body" innerHTML={renderMarkdown(m.content)} />
              </div>
            </>
          )}
        </For>

        {/* The in-flight turn: live tool summary, then the streaming reply. */}
        <Show when={runningTurn()}>{renderSummary(runningTurn())}</Show>
        <Show when={bubbleText()}>
          <div class="msg assistant pending">
            <span class="role">assistant · streaming</span>
            {/* The streaming binding — accessor form so Solid re-evaluates
                renderMarkdown on every `assistant_delta` that grows
                streaming.text. No createMemo needed; re-parsing each delta
                is cheap and avoids dependency-tracking surprises (spec
                M2-polish §B "streaming 渲染陷阱"). */}
            <div
              class="msg-body markdown-body"
              innerHTML={renderMarkdown(bubbleText())}
            />
          </div>
        </Show>
        <Show when={props.sending() && !bubbleText()}>
          <div class="msg assistant pending thinking">
            <span class="role">assistant</span>
            <p>处理中…</p>
          </div>
        </Show>
      </div>

      <form
        class="composer"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <textarea
          rows={2}
          placeholder="向 agent 提一个研究问题（Enter 发送，Shift+Enter 换行）"
          value={draft()}
          onInput={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <button type="submit">发送</button>
      </form>
    </section>
  );
}
