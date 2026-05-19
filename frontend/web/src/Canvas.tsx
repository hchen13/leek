// Canvas — the read-only execution-trace workbench (REQUIREMENTS §2.2).
//
// The session's process artifacts, segmented by turn. Each turn section
// lists its Note Trace / tool / search cards in event order. Failed tool
// cards are hidden by default and revealed by a settings toggle (§2.4).
// The canvas never shows the final reply — that lives only in chat.

import { createMemo, createSignal, For, Show } from "solid-js";

import type { Artifact, Message, Turn } from "./types";

type CanvasProps = {
  turns: () => Turn[];
  messages: () => Message[];
  showFailed: () => boolean;
  setShowFailed: (v: boolean) => void;
  /** artifactId currently flashed by a chat tool-summary click. */
  highlight: () => string | null;
};

/** A failed tool card — hidden from the linear flow unless `showFailed`. */
function isHiddenFailure(a: Artifact, showFailed: boolean): boolean {
  return a.kind === "tool" && a.phase === "error" && !showFailed;
}

type RenderItem =
  | { type: "notes"; key: string; notes: Artifact[] }
  | { type: "card"; key: string; artifact: Artifact };

/** Group a turn's visible artifacts: adjacent notes coalesce into one Note
 *  Trace card (§2.3); a tool / search card separates note runs. */
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

const STOP_LABELS: Record<string, string> = {
  end_turn: "完成",
  max_tokens: "长度上限",
  idle_timeout: "空闲超时",
  wall_clock_exceeded: "时间上限",
  max_iterations: "迭代上限",
  cost_cap_exceeded: "成本上限",
  doom_loop: "工具循环",
  fatal_error: "出错",
};

function stopLabel(reason: string | null): string {
  if (!reason) return "完成";
  return STOP_LABELS[reason] ?? reason;
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

function snippet(text: string, max = 64): string {
  const one = text.replace(/\s+/g, " ").trim();
  return one.length <= max ? one : one.slice(0, max) + "…";
}

function formatJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function Canvas(props: CanvasProps) {
  // Card-level UI state, keyed by id so it survives re-renders.
  const [expanded, setExpanded] = createSignal<Set<string>>(new Set());
  const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());

  const toggle = (set: () => Set<string>, put: (s: Set<string>) => void, id: string) => {
    const next = new Set(set());
    if (next.has(id)) next.delete(id);
    else next.add(id);
    put(next);
  };

  const hiddenFailures = createMemo(() => {
    if (props.showFailed()) return 0;
    let n = 0;
    for (const t of props.turns()) {
      for (const a of t.artifacts) if (a.kind === "tool" && a.phase === "error") n += 1;
    }
    return n;
  });

  const renderToolBody = (a: Artifact) => {
    if (a.phase === "start") {
      return <p class="card-running">运行中…</p>;
    }
    if (a.phase === "error") {
      const err = (a.displayPayload?.error as string) ?? "工具调用失败";
      return <p class="card-error">✗ {err}</p>;
    }
    const dp = a.displayPayload ?? {};
    if (a.cardKind === "text") {
      return <pre class="card-pre">{String(dp.text ?? "")}</pre>;
    }
    if (a.cardKind === "web_preview") {
      const url = String(dp.url ?? "");
      const host = String(dp.host ?? "");
      const title = dp.title ? String(dp.title) : host || url;
      return (
        <div class="web-preview">
          <a class="web-title" href={url} target="_blank" rel="noreferrer">
            {title}
          </a>
          <div class="muted">
            {host} · HTTP {String(dp.status ?? "?")} · {String(dp.char_count ?? "?")} 字符
            {dp.truncated ? " · 已截断" : ""}
          </div>
        </div>
      );
    }
    return <pre class="card-pre">{formatJson(dp)}</pre>;
  };

  const renderToolCard = (a: Artifact) => {
    const badge =
      a.phase === "error" ? "failed" : a.phase === "start" ? "running" : "ok";
    return (
      <div
        id={`card-${a.artifactId}`}
        classList={{ card: true, "tool-card": true, highlighted: props.highlight() === a.artifactId }}
      >
        <div class="card-head">
          <span classList={{ "status-dot": true, [badge]: true }} />
          <span class="card-name">{a.displayName ?? a.tool ?? "工具"}</span>
          <span class="card-summary">{a.summary ?? ""}</span>
          <button class="card-expand" onClick={() => toggle(expanded, setExpanded, a.artifactId)}>
            {expanded().has(a.artifactId) ? "收起" : "详情"}
          </button>
        </div>
        {renderToolBody(a)}
        <Show when={expanded().has(a.artifactId)}>
          <pre class="card-debug">{formatJson({ arguments: a.args, debug: a.debugPayload })}</pre>
        </Show>
      </div>
    );
  };

  const renderSearchCard = (a: Artifact) => (
    <div
      id={`card-${a.artifactId}`}
      classList={{ card: true, "search-card": true, highlighted: props.highlight() === a.artifactId }}
    >
      <div class="card-head">
        <span classList={{ "status-dot": true, running: a.phase === "start", ok: a.phase !== "start" }} />
        <span class="card-name">网页搜索</span>
        <span class="card-summary">{a.query ?? ""}</span>
      </div>
      <Show when={a.phase === "start"}>
        <p class="card-running">搜索中…</p>
      </Show>
      <Show when={a.phase !== "start"}>
        <Show
          when={a.sources && a.sources.length > 0}
          fallback={<p class="muted">（本次搜索没有返回可引用来源）</p>}
        >
          <ul class="source-list">
            <For each={a.sources}>
              {(s) => (
                <li>
                  <a href={s.url} target="_blank" rel="noreferrer">
                    {s.title ?? s.host ?? s.url}
                  </a>
                  <Show when={s.host}>
                    <span class="muted"> · {s.host}</span>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Show>
    </div>
  );

  const renderNotes = (notes: Artifact[]) => (
    <div class="card note-card">
      <div class="card-label">Note Trace</div>
      <For each={notes}>{(n) => <p class="note-text">{n.text}</p>}</For>
    </div>
  );

  const renderItem = (item: RenderItem) => {
    if (item.type === "notes") return renderNotes(item.notes);
    const a = item.artifact;
    return a.kind === "search" ? renderSearchCard(a) : renderToolCard(a);
  };

  const renderTurn = (turn: Turn, index: number) => {
    const groups = createMemo(() => groupArtifacts(turn.artifacts, props.showFailed()));
    const tools = () => turn.artifacts.filter((a) => a.kind === "tool" || a.kind === "search").length;
    const fails = () =>
      turn.artifacts.filter((a) => a.kind === "tool" && a.phase === "error").length;
    const hasNote = () => turn.artifacts.some((a) => a.kind === "note");
    const isCollapsed = () => collapsed().has(turn.turnId);

    return (
      <section class="turn-section">
        <header class="turn-head" onClick={() => toggle(collapsed, setCollapsed, turn.turnId)}>
          <span class="turn-toggle">{isCollapsed() ? "▸" : "▾"}</span>
          <span class="turn-no">回合 {index + 1}</span>
          <span class="turn-q">{snippet(userQuestion(turn, props.messages()))}</span>
          <span class="turn-meta">
            <Show when={turn.status === "running"} fallback={stopLabel(turn.stopReason)}>
              进行中…
            </Show>
            {" · "}
            {tools()} 工具
            <Show when={fails() > 0}>
              <span class="meta-fail"> · {fails()} 失败</span>
            </Show>
            <Show when={hasNote()}>{" · 有 note"}</Show>
            <Show when={turn.compactions > 0}>{` · 压缩 ${turn.compactions}`}</Show>
            <Show when={turn.metrics}>
              {` · ${(turn.metrics!.wallClockMs / 1000).toFixed(1)}s`}
            </Show>
          </span>
        </header>
        <Show when={!isCollapsed()}>
          <div class="turn-body">
            <Show when={turn.error}>
              <div class="card turn-error">✗ {turn.error}</div>
            </Show>
            <For
              each={groups()}
              fallback={<p class="muted">（本回合没有可见的过程卡片）</p>}
            >
              {(item) => renderItem(item)}
            </For>
          </div>
        </Show>
      </section>
    );
  };

  const anyArtifacts = () => props.turns().some((t) => t.artifacts.length > 0);

  return (
    <section class="canvas">
      <header class="panel-head">
        <h2>Canvas</h2>
        <label class="canvas-setting">
          <input
            type="checkbox"
            checked={props.showFailed()}
            onChange={(e) => props.setShowFailed(e.currentTarget.checked)}
          />
          显示失败的工具调用
          <Show when={hiddenFailures() > 0 && !props.showFailed()}>
            <span class="muted"> ({hiddenFailures()})</span>
          </Show>
        </label>
      </header>
      <div class="canvas-scroll">
        <Show
          when={anyArtifacts()}
          fallback={<p class="muted canvas-empty">本会话还没有过程卡片。发送一条消息后，agent 的 Note Trace、工具和搜索过程会出现在这里。</p>}
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
