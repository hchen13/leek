// CanvasColumn — the read-only execution-trace surface (REQUIREMENTS §2.2).
//
// Phase 1 replaces the old monolithic Canvas.tsx with a token-cascaded
// shell. Each turn is a sub-section; visible artifacts inside the turn
// render as GenericToolCard instances (Phase 2 swaps to per-tool renderers).
// The column scrolls; cards are LEFT-aligned (variable-width by `.lk-card--*`).
//
// State coverage:
//   - empty session → friendly placeholder
//   - turn with no visible cards (all failed + hidden) → muted note
//   - failed cards hidden by default; toggle in header reveals them
//   - highlight class propagates to flash a card from chat summary click

import { createMemo, createSignal, For, Show } from "solid-js";

import { GenericToolCard } from "./GenericToolCard";
import { SafeMarkdown } from "./SafeMarkdown";
import type { Artifact, Message, Turn } from "../types";

type Props = {
  turns: () => Turn[];
  messages: () => Message[];
  showFailed: () => boolean;
  setShowFailed: (v: boolean) => void;
  /** artifactId currently flashed by chat tool-summary click. */
  highlight: () => string | null;
  /** Session title — small breadcrumb at the top. */
  sessionTitle: () => string;
};

const STOP_LABELS: Record<string, string> = {
  end_turn: "完成",
  max_tokens: "长度上限",
  idle_timeout: "空闲超时",
  wall_clock_exceeded: "时间上限",
  max_iterations: "迭代上限",
  cost_cap_exceeded: "成本上限",
  doom_loop: "工具循环",
  codex_duplicate_abort: "codex 重复 URL",
  fatal_error: "出错",
};

function stopLabel(reason: string | null): string {
  if (!reason) return "完成";
  return STOP_LABELS[reason] ?? reason;
}

function isHiddenFailure(a: Artifact, showFailed: boolean): boolean {
  return a.kind === "tool" && a.phase === "error" && !showFailed;
}

/** Adjacent notes coalesce into a single visual block so the canvas
 *  doesn't look like spam when an agent emits 5 quick notes between two
 *  tool calls. */
type RenderItem =
  | { type: "notes"; key: string; notes: Artifact[] }
  | { type: "card"; key: string; artifact: Artifact };

function groupArtifacts(arts: Artifact[], showFailed: boolean): RenderItem[] {
  const items: RenderItem[] = [];
  for (const a of arts) {
    if (isHiddenFailure(a, showFailed)) continue;
    if (a.kind === "note") {
      const last = items[items.length - 1];
      if (last && last.type === "notes") last.notes.push(a);
      else items.push({ type: "notes", key: a.artifactId, notes: [a] });
    } else {
      items.push({ type: "card", key: a.artifactId, artifact: a });
    }
  }
  return items;
}

function snippet(text: string, max = 80): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length <= max ? one : one.slice(0, max) + "…";
}

function userQuestion(turn: Turn, messages: Message[]): string {
  if (turn.messageSeq != null) {
    const m = messages.find((x) => x.seq === turn.messageSeq! - 1 && x.role === "user");
    if (m) return m.content;
  }
  if (turn.status === "running") {
    const users = messages.filter((m) => m.role === "user");
    if (users.length > 0) return users[users.length - 1].content;
  }
  return "";
}

export function CanvasColumn(props: Props) {
  const hiddenFailures = createMemo(() => {
    if (props.showFailed()) return 0;
    let n = 0;
    for (const t of props.turns()) {
      for (const a of t.artifacts) {
        if (a.kind === "tool" && a.phase === "error") n += 1;
      }
    }
    return n;
  });

  const anyArtifacts = () => props.turns().some((t) => t.artifacts.length > 0);

  const renderNotes = (notes: Artifact[]) => (
    <div class="lk-card lk-card--md lk-card--note">
      <div class="lk-card-note-label">Note Trace</div>
      <For each={notes}>
        {(n) => <SafeMarkdown text={n.text ?? ""} class="lk-card-md" />}
      </For>
    </div>
  );

  const renderItem = (item: RenderItem) => {
    if (item.type === "notes") return renderNotes(item.notes);
    const a = item.artifact;
    return (
      <GenericToolCard
        artifact={a}
        highlighted={props.highlight() === a.artifactId}
      />
    );
  };

  const renderTurn = (turn: Turn, index: number) => {
    const groups = createMemo(() => groupArtifacts(turn.artifacts, props.showFailed()));
    const tools = () =>
      turn.artifacts.filter((a) => a.kind === "tool" || a.kind === "search").length;
    const fails = () =>
      turn.artifacts.filter((a) => a.kind === "tool" && a.phase === "error").length;

    return (
      <section class="lk-turn">
        <header class="lk-turn-head">
          <span class="lk-turn-no lk-num">回合 {index + 1}</span>
          <span class="lk-turn-q">{snippet(userQuestion(turn, props.messages()))}</span>
          <span class="lk-turn-meta">
            <Show when={turn.status === "running"} fallback={stopLabel(turn.stopReason)}>
              进行中…
            </Show>
            <span class="lk-turn-meta-sep">·</span>
            <span class="lk-num">{tools()} 工具</span>
            <Show when={fails() > 0}>
              <span class="lk-turn-meta-sep">·</span>
              <span class="lk-turn-meta-fail lk-num">{fails()} 失败</span>
            </Show>
            <Show when={turn.metrics}>
              <span class="lk-turn-meta-sep">·</span>
              <span class="lk-num">
                {(turn.metrics!.wallClockMs / 1000).toFixed(1)}s
              </span>
            </Show>
          </span>
        </header>
        <div class="lk-turn-body">
          <Show when={turn.error}>
            <div class="lk-bar lk-bar--danger">✗ {turn.error}</div>
          </Show>
          <For
            each={groups()}
            fallback={
              <p class="lk-empty-inline">本回合没有可见过程卡片</p>
            }
          >
            {(item) => renderItem(item)}
          </For>
        </div>
      </section>
    );
  };

  return (
    <section class="lk-canvas" aria-label="Canvas">
      <header class="lk-canvas-head">
        <span class="lk-canvas-crumb" title={props.sessionTitle()}>
          {props.sessionTitle()}
        </span>
        <span class="lk-canvas-head-spacer" />
        <label class="lk-canvas-toggle">
          <input
            type="checkbox"
            checked={props.showFailed()}
            onChange={(e) => props.setShowFailed(e.currentTarget.checked)}
          />
          <span>显示失败的工具调用</span>
          <Show when={hiddenFailures() > 0 && !props.showFailed()}>
            <span class="lk-muted lk-num">({hiddenFailures()})</span>
          </Show>
        </label>
      </header>
      <div class="lk-canvas-scroll">
        <Show
          when={anyArtifacts()}
          fallback={
            <div class="lk-empty lk-canvas-empty">
              <span class="lk-empty-title">本会话还没有过程卡片</span>
              <span class="lk-empty-hint">
                发送一条消息后,agent 的 Note Trace、工具和搜索过程会在这里。
              </span>
            </div>
          }
        >
          <For each={props.turns()}>
            {(turn, i) => (
              <Show when={turn.artifacts.length > 0}>{renderTurn(turn, i())}</Show>
            )}
          </For>
        </Show>
      </div>
    </section>
  );
}
