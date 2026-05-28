// ChatColumn — user prompts, assistant final replies, composer.
//
// Phase 1 column (380px). Contents:
//   - Header: history button (left) · session title (centre) · new session (right)
//   - Body:   bubble stream (user / assistant) — markdown-rendered
//   - Footer: status pill (visible while a turn runs) + composer textarea
//
// Note Trace and tool / search bodies do NOT appear in this column —
// they belong to Canvas (REQUIREMENTS §2.1 / §2.2). What we DO show here
// is a compact per-turn summary (tool / search calls coalesced into one
// "已执行 N 步" line) the user can expand to see clickable artifacts
// that jump into the corresponding canvas card.
//
// State coverage:
//   - no session selected → empty body with friendly placeholder
//   - empty session       → "还没有消息" placeholder
//   - turn running        → status pill + streaming bubble
//   - assistant message   → static bubble
//   - cost-cap soft-stop  → cost-cap bar after the message
//   - fatal_reason set    → fatal hint card with retry action

import { createSignal, For, Show } from "solid-js";

import { Icon } from "./Icon";
import { SafeMarkdown } from "./SafeMarkdown";
import { StatusPill } from "./StatusPill";
import type { Message, Turn } from "../types";

type Streaming = { turnId: string; iteration: number; text: string };

type SummaryStatus = "running" | "ok" | "error";
type SummaryItem = {
  artifactId: string;
  label: string;
  detail: string;
  status: SummaryStatus;
};

function summaryItems(turn: Turn): SummaryItem[] {
  return turn.artifacts
    .filter((a) => a.kind !== "note")
    .map((a) => ({
      artifactId: a.artifactId,
      label:
        a.kind === "search"
          ? "网页搜索"
          : a.kind === "subagent"
            ? `委派 ${a.agentName ?? "subagent"}`
            : a.displayName ?? a.tool ?? "工具",
      detail: a.kind === "search" ? a.query ?? "" : a.summary ?? "",
      status: a.phase === "start" ? "running" : a.phase === "error" ? "error" : "ok",
    }));
}

function statusGlyph(s: SummaryStatus): string {
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

function shouldShowCostCapBar(turn: Turn): boolean {
  return turn.stopReason === "cost_cap_exceeded" && turn.costCap != null;
}

function fmtUsd(n: number): string {
  return `$${n.toFixed(2)}`;
}

function shouldShowFatalCard(turn: Turn): boolean {
  return turn.fatalReason != null;
}

type Props = {
  messages: () => Message[];
  turns: () => Turn[];
  streaming: () => Streaming | null;
  noted: () => Record<string, true>;
  sending: () => boolean;
  send: (text: string) => void;
  focusCard: (artifactId: string, isError: boolean) => void;
  openSettings: () => void;
  sessionId: () => string | null;
  /** Session title to render in the header (centered). */
  sessionTitle: () => string;
  /** Open / close the session drawer. */
  onOpenDrawer: () => void;
  /** Spawn a new session. */
  onNewSession: () => void;
};

export function ChatColumn(props: Props) {
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

  const bubbleText = (): string => {
    const s = props.streaming();
    if (!s) return "";
    if (props.noted()[`${s.turnId}:${s.iteration}`]) return "";
    return s.text;
  };

  const renderCostCapBar = (turn: Turn | undefined) => {
    if (!turn || !shouldShowCostCapBar(turn)) return null;
    const cap = turn.costCap!;
    return (
      <div class="lk-bar lk-bar--warn" role="alert">
        <span class="lk-bar-icon" aria-hidden="true">!</span>
        <span class="lk-bar-text">
          本轮研究达到预算上限 <span class="lk-num">{fmtUsd(cap.capUsd)}</span>
          (实际 <span class="lk-num">{fmtUsd(cap.actualCostUsd)}</span>),已在第 {cap.iterCount} 步停止。
        </span>
        <button class="lk-bar-link" onClick={() => props.openSettings()} type="button">
          打开设置 →
        </button>
      </div>
    );
  };

  const priorUserContent = (assistantSeq: number): string | null => {
    const msgs = props.messages();
    const idx = msgs.findIndex((m) => m.seq === assistantSeq);
    if (idx <= 0) return null;
    for (let i = idx - 1; i >= 0; i--) {
      if (msgs[i].role === "user") return msgs[i].content;
    }
    return null;
  };

  const renderFatalCard = (turn: Turn | undefined, assistantSeq: number) => {
    if (!turn || !shouldShowFatalCard(turn)) return null;
    const fr = turn.fatalReason!;
    const retryContent = () => priorUserContent(assistantSeq);
    const onRetry = () => {
      const content = retryContent();
      if (!content) return;
      props.send(content);
    };
    return (
      <div class="lk-bar lk-bar--warn lk-bar--fatal" role="alert">
        <span class="lk-bar-icon" aria-hidden="true">!</span>
        <div class="lk-bar-body">
          <p class="lk-bar-text">{fr.hint}</p>
          <p class="lk-bar-detail lk-mono">
            <span>{fr.kind}</span>
            <Show when={fr.detail}>
              <span> · {fr.detail}</span>
            </Show>
          </p>
          <div class="lk-bar-actions">
            <Show when={retryContent() != null}>
              <button
                class="lk-btn lk-btn--secondary lk-btn--sm"
                onClick={onRetry}
                disabled={props.sending()}
                type="button"
                title="把上一条 user 消息原样再发一次"
              >
                重试本回合
              </button>
            </Show>
            <span class="lk-bar-skip">继续提问(跳过) →</span>
          </div>
        </div>
      </div>
    );
  };

  const renderSummary = (turn: Turn | undefined) => {
    if (!turn) return null;
    const items = summaryItems(turn);
    if (items.length === 0) return null;
    const running = turn.status === "running";
    const open = () => running || expanded().has(turn.turnId);
    return (
      <div class="lk-tool-summary">
        <Show when={!running}>
          <button
            class="lk-tool-aggregate"
            type="button"
            onClick={() => toggleSummary(turn.turnId)}
          >
            <span class="lk-tool-caret">{open() ? "▾" : "▸"}</span>
            <span>{aggregateText(items)}</span>
          </button>
        </Show>
        <Show when={open()}>
          <ul class="lk-tool-list">
            <For each={items}>
              {(it) => (
                <li
                  classList={{
                    "lk-tool-item": true,
                    [`lk-tool-item--${it.status}`]: true,
                  }}
                  onClick={() => props.focusCard(it.artifactId, it.status === "error")}
                  title="跳转到 canvas 卡片"
                >
                  <span class="lk-tool-icon">{statusGlyph(it.status)}</span>
                  <span class="lk-tool-label">{it.label}</span>
                  <Show when={it.detail}>
                    <span class="lk-tool-detail">{it.detail}</span>
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
    <section class="lk-chat" aria-label="Chat">
      <header class="lk-chat-head">
        <button
          class="lk-icon-btn"
          onClick={() => props.onOpenDrawer()}
          type="button"
          title="历史会话"
          aria-label="历史会话"
        >
          <Icon name="history" size={16} />
        </button>
        <h2 class="lk-chat-title" title={props.sessionTitle()}>
          {props.sessionTitle()}
        </h2>
        <button
          class="lk-icon-btn"
          onClick={() => props.onNewSession()}
          type="button"
          title="新建会话"
          aria-label="新建会话"
        >
          <Icon name="plus" size={16} />
        </button>
      </header>

      <div class="lk-chat-body">
        <For
          each={props.messages()}
          fallback={
            <p class="lk-empty">
              <span class="lk-empty-title">还没有消息</span>
              <span class="lk-empty-hint">在下方输入一个研究问题开始。</span>
            </p>
          }
        >
          {(m) => (
            <>
              <Show when={m.role === "assistant"}>
                {renderSummary(turnForMessage(m.seq))}
              </Show>
              <div
                classList={{
                  "lk-bubble": true,
                  [`lk-bubble--${m.role}`]: true,
                }}
              >
                <SafeMarkdown text={m.content} />
              </div>
              <Show when={m.role === "assistant"}>
                {renderCostCapBar(turnForMessage(m.seq))}
              </Show>
              <Show when={m.role === "assistant"}>
                {renderFatalCard(turnForMessage(m.seq), m.seq)}
              </Show>
            </>
          )}
        </For>

        <Show when={runningTurn()}>{renderSummary(runningTurn())}</Show>
        <Show when={bubbleText()}>
          <div class="lk-bubble lk-bubble--assistant lk-bubble--streaming">
            <SafeMarkdown text={bubbleText()} />
          </div>
        </Show>
        <Show when={props.sending() && !bubbleText()}>
          <div class="lk-bubble lk-bubble--assistant lk-bubble--streaming">
            <span class="lk-bubble-thinking">处理中…</span>
          </div>
        </Show>
      </div>

      <StatusPill turn={() => runningTurn() ?? null} sessionId={props.sessionId} />

      <form
        class="lk-composer"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <textarea
          class="lk-composer-input"
          rows={2}
          placeholder="向 agent 提问(Enter 发送, Shift+Enter 换行)"
          value={draft()}
          onInput={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <button
          class="lk-composer-send"
          type="submit"
          disabled={props.sending() || draft().trim().length === 0}
          title="发送"
          aria-label="发送"
        >
          <Icon name="send" size={16} />
        </button>
      </form>
    </section>
  );
}
