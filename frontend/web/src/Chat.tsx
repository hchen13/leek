// Chat — the user's input and final-reply column (REQUIREMENTS §2.1).
//
// Chat holds only user messages, assistant final replies, and a compact
// tool / progress summary per turn. Note Trace never appears here — it is
// canvas content. While a turn runs the summary lists each tool / search
// call live; once the turn ends it folds to one aggregate line that
// expands again on click. Clicking a summary item focuses its canvas card.

import { createSignal, For, Show } from "solid-js";

import { renderMarkdown } from "./markdown";
import { TurnStatusPill } from "./TurnStatusPill";
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
  /** M2.6: clicked from the cost-cap warning bar's "打开 Settings →" link. */
  openSettings: () => void;
  /** M3.2: current session id, plumbed down so the status pill's abort
   *  button can POST to the right endpoint. `null` only on the empty
   *  fallback screen — when no session is selected the pill is hidden. */
  sessionId: () => string | null;
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

/** Show the cost-cap warning bar iff the turn was soft-stopped by the
 *  cap (M2.6). The check leans on `stopReason === "cost_cap_exceeded"`
 *  rather than just `costCap != null`, because in principle a future
 *  event could carry cap telemetry without aborting the turn. Today the
 *  two coincide. */
function shouldShowCostCapBar(turn: Turn): boolean {
  return turn.stopReason === "cost_cap_exceeded" && turn.costCap != null;
}

/** Pretty-print a USD amount: $0.50 for cents-sized values, $1.23 otherwise.
 *  We always show 2 decimals so a cap of `$0.5` doesn't render as `$0.5`
 *  next to an actual spend of `$0.61` (visually inconsistent). */
function fmtUsd(n: number): string {
  return `$${n.toFixed(2)}`;
}

/** M3.3: show the fatal-reason hint card iff the turn carries a typed
 *  reason. Populated for `fatal_error` stops (HTTP / connection /
 *  malformed) AND for `idle_timeout` (the substantive-only detector
 *  trips with a `codex_stream_silent` kind). Other guard-only stops
 *  (cost_cap / wall_clock / max_iter / user_aborted) have their own
 *  bars or render via the persisted message's tail note. */
function shouldShowFatalCard(turn: Turn): boolean {
  return turn.fatalReason != null;
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

  /** M2.6: render the cost-cap warning bar for a finished turn that hit
   *  the cap. Placed at the *end* of the turn — after its summary and
   *  any final assistant message — so it reads as "this is why the turn
   *  stopped here", not as a header to what follows. Visible until the
   *  user starts a new turn (which simply puts a new turn after it). */
  const renderCostCapBar = (turn: Turn | undefined) => {
    if (!turn || !shouldShowCostCapBar(turn)) return null;
    const cap = turn.costCap!;
    return (
      <div class="cost-cap-bar" role="alert">
        <span class="cost-cap-icon" aria-hidden="true">
          ⚠
        </span>
        <span class="cost-cap-text">
          本轮研究达到预算上限 {fmtUsd(cap.capUsd)}（实际 {fmtUsd(cap.actualCostUsd)}），
          已在第 {cap.iterCount} 步停止。 再问一句可以继续 / 或在 Settings 调整预算
        </span>
        <button class="cost-cap-link" onClick={() => props.openSettings()}>
          打开 Settings →
        </button>
      </div>
    );
  };

  /** M3.3: render the typed fatal-reason hint card for a turn that died
   *  on the codex side (HTTP error / connection / malformed) or sat
   *  silent past the idle budget. Yellow-bordered (the majority of
   *  these are "retry usually fixes it" — not red-alert fatal). The
   *  hint copy is owned by the backend (`FatalReason::hint`); the card
   *  is a dumb renderer.
   *
   *  Placed after the assistant message — same slot as the cost-cap bar
   *  — so the reading order is "model said X, then this is why it
   *  stopped". A turn that hits both the cost cap AND a fatal would
   *  show both; today the loop can only hit one. */
  const renderFatalCard = (turn: Turn | undefined) => {
    if (!turn || !shouldShowFatalCard(turn)) return null;
    const fr = turn.fatalReason!;
    return (
      <div class="fatal-hint-card" role="alert">
        <div class="fatal-hint-icon" aria-hidden="true">
          ⚠
        </div>
        <div class="fatal-hint-body">
          <p class="fatal-hint-text">{fr.hint}</p>
          <p class="fatal-hint-detail">
            <span class="fatal-hint-kind">{fr.kind}</span>
            <span class="fatal-hint-detail-text">{fr.detail}</span>
          </p>
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
              {/* M2.6: cost-cap warning bar at turn end. Lives after the
                  assistant message so it reads as a postscript to the
                  reply that the cap forced to stop early. */}
              <Show when={m.role === "assistant"}>{renderCostCapBar(turnForMessage(m.seq))}</Show>
              {/* M3.3: typed fatal-reason hint card — same slot. Renders
                  for fatal_error stops (HTTP / connection / malformed)
                  and idle_timeout (substantive-only idle detector trip
                  → codex_stream_silent). Empty / null payload short-
                  circuits inside the renderer. */}
              <Show when={m.role === "assistant"}>{renderFatalCard(turnForMessage(m.seq))}</Show>
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

      {/* M3.2: reasoning status pill — between the messages and the
          composer, only visible while a turn is in flight. Provides the
          "agent is alive" signal during long reasoning + builtin
          web_search phases, plus the abort affordance. */}
      <TurnStatusPill turn={() => runningTurn() ?? null} sessionId={props.sessionId} />

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
