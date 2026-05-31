// Tool-specific canvas cards. Each tool kind gets its own renderer that
// presents the output as the user wants to *see* it (a wiki abstract, a
// candlestick chart, a link preview) rather than raw JSON. When we can't parse
// the truncated output (or the tool isn't specifically handled yet), a
// generic fallback card shows the bare facts.

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { CandlestickSeries, createChart, type IChartApi, type ISeriesApi } from "lightweight-charts";
import { SafeMarkdown } from "./SafeMarkdown";

export interface ToolCallView {
  call_id: string;
  status: "in_progress" | "completed" | "error" | string;
  name: string;
  arguments?: string;
  output_preview?: string;
  output_bytes?: number;
  ui_artifact?: ToolUiArtifact;
  cached?: boolean;
  cached_from?: string;
  error_kind?: string;
  retryable?: boolean;
  partial_status?: string | null;
}

export interface ToolUiArtifact {
  version?: number;
  tool_name?: string;
  arguments?: unknown;
  content_type?: "json" | "markdown" | "text" | string;
  payload?: unknown;
  output_bytes?: number;
}

export interface SearchCallView {
  status: string;
  action: string;
  detail: string;
  queries?: string[];
  sources?: string[];
}

/** Match LiveChat's `searchKey` exactly so chat clicks resolve to the same
 *  data-call-id on the canvas tile. Keep these in sync. */
function searchKey(s: SearchCallView): string {
  const detail = s.detail || s.queries?.[0] || "";
  return `search:${s.action}:${detail.slice(0, 120)}`;
}

export interface NarrationStep {
  turn: number;
  text: string;
  seq?: number;
}

export interface DecisionDraftView {
  deliverable_id: string;
  ticker: string;
  direction: string;
  size_pct?: number;
  stop_loss?: number;
  target?: number;
  horizon_days?: number;
  rationale: string;
}

export interface ArtifactEventView {
  id: string;
  kind: "narration" | "narration_group" | "search" | "tool" | "decision";
  turn?: number;
  narration?: NarrationStep;
  narrations?: NarrationStep[];
  search?: SearchCallView;
  tool?: ToolCallView;
  decision?: DecisionDraftView;
}

interface ArtifactCallbacks {
  onOpenDoc?: (id: string, title: string) => void;
}

export function ArtifactPanel(props: {
  searches: SearchCallView[];
  tools: ToolCallView[];
  narrations?: NarrationStep[];
  events?: ArtifactEventView[];
  currentTurn?: number;
  callbacks?: ArtifactCallbacks;
}) {
  const [showAllTurns, setShowAllTurns] = createSignal(false);
  const events = createMemo<ArtifactEventView[]>(() => {
    const source = props.events && props.events.length > 0 ? props.events : [
      ...(props.narrations ?? []).map((n, i) => ({
        id: `narration:${n.turn}:${i}`,
        kind: "narration" as const,
        narration: n,
      })),
      ...props.searches.map((s) => ({
        id: searchKey(s),
        kind: "search" as const,
        search: s,
      })),
      ...props.tools.map((t) => ({
        id: t.call_id,
        kind: "tool" as const,
        tool: t,
      })),
    ];

    const out: ArtifactEventView[] = [];
    const seenNarration = new Set<string>();
    let narrationSeq = 0;
    for (const event of source) {
      if (event.kind === "narration" && event.narration) {
        const text = normalizeNarrationText(event.narration.text);
        if (!text || seenNarration.has(text)) continue;
        seenNarration.add(text);
        narrationSeq += 1;
        out.push({ ...event, narration: { ...event.narration, seq: narrationSeq } });
      } else if (event.kind === "narration_group" && event.narrations) {
        const narrations: NarrationStep[] = [];
        for (const narration of event.narrations) {
          const text = normalizeNarrationText(narration.text);
          if (!text || seenNarration.has(text)) continue;
          seenNarration.add(text);
          narrationSeq += 1;
          narrations.push({ ...narration, seq: narrationSeq });
        }
        if (narrations.length > 0) out.push({ ...event, narrations });
      } else if (event.kind === "tool" && event.tool) {
        upsertToolEvent(out, { ...event, id: event.tool.call_id });
      } else {
        out.push(event);
      }
    }
    return out;
  });
  const groupedEvents = createMemo<ArtifactEventView[]>(() => {
    const out: ArtifactEventView[] = [];
    for (const event of events()) {
      if (event.kind === "narration" && event.narration) {
        const last = out[out.length - 1];
        if (last?.kind === "narration_group" && last.turn === event.turn) {
          last.narrations = [...(last.narrations ?? []), event.narration];
        } else {
          out.push({
            id: event.id,
            kind: "narration_group",
            turn: event.turn,
            narrations: [event.narration],
          });
        }
      } else {
        out.push(event);
      }
    }
    return out;
  });
  const latestArtifactTurn = createMemo(() =>
    groupedEvents().reduce((max, event) => Math.max(max, event.turn ?? 0), 0)
  );
  const latestTurn = createMemo(() => Math.max(props.currentTurn ?? 0, latestArtifactTurn()));
  const visibleEvents = createMemo(() => {
    const latest = latestTurn();
    if (showAllTurns() || latest === 0) return groupedEvents();
    return groupedEvents().filter((event) => event.turn === latest);
  });

  // `<For>` reconciles by object identity, but every streaming tick rebuilds the
  // event objects from scratch (messages() → sessionArtifactEvents() → events()
  // all spread copies). Without identity stability the card rows get torn down
  // and recreated on each delta, resetting card-local state — open modals,
  // selected tab/periods. Reuse the previous object for any event whose content
  // is unchanged so `<For>` keeps the live row, and its state, intact.
  const stableCache = new Map<string, { sig: string; value: ArtifactEventView }>();
  const stableEvents = createMemo(() => {
    const next = visibleEvents();
    const live = new Set<string>();
    const out = next.map((event) => {
      live.add(event.id);
      const sig = JSON.stringify(event);
      const cached = stableCache.get(event.id);
      if (cached && cached.sig === sig) return cached.value;
      stableCache.set(event.id, { sig, value: event });
      return event;
    });
    for (const key of stableCache.keys()) if (!live.has(key)) stableCache.delete(key);
    return out;
  });

  return (
    <div class="lk-artifact-grid">
      <Show when={latestTurn() > 0}>
        <div class="lk-artifact-scope">
          <span>第 {latestTurn()} 轮</span>
          <button type="button" class={!showAllTurns() ? "active" : ""} onClick={() => setShowAllTurns(false)}>当前</button>
          <button type="button" class={showAllTurns() ? "active" : ""} onClick={() => setShowAllTurns(true)}>全部</button>
        </div>
      </Show>
      <For each={stableEvents()}>{(event) => (
        <ArtifactEventCard event={event} callbacks={props.callbacks} />
      )}</For>
      <Show when={!showAllTurns() && latestTurn() > 0 && visibleEvents().length === 0}>
        <div class="lk-artifact-card is-wide" style={{
          background: "rgba(255,255,255,0.018)",
          border: "1px solid var(--line-1)",
          "border-radius": "8px",
          padding: "14px 16px",
          color: "var(--ink-3)",
          "font-size": "13px",
        }}>
          当前轮没有新的工具或网页证据，回答沿用了前文上下文。
        </div>
      </Show>
    </div>
  );
}

function normalizeNarrationText(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

function mergeToolCall(prev: ToolCallView, next: ToolCallView): ToolCallView {
  const merged = {
    ...prev,
    ...next,
    call_id: prev.call_id || next.call_id,
    arguments: prev.arguments ?? next.arguments,
    output_preview: next.output_preview ?? prev.output_preview,
    output_bytes: next.output_bytes ?? prev.output_bytes,
    ui_artifact: next.ui_artifact ?? prev.ui_artifact,
    cached: next.cached ?? prev.cached,
    cached_from: next.cached_from ?? prev.cached_from,
    error_kind: next.error_kind ?? prev.error_kind,
    retryable: next.retryable ?? prev.retryable,
    partial_status: next.partial_status ?? prev.partial_status,
  };
  if (next.status === "in_progress" && prev.status !== "in_progress") merged.status = prev.status;
  return merged;
}

function upsertToolEvent(out: ArtifactEventView[], event: ArtifactEventView) {
  const tool = event.tool;
  if (!tool) return;
  const toolEvents = out
    .map((existing, index) => ({ existing, index }))
    .filter(({ existing }) => existing.kind === "tool" && existing.tool);
  const idx = toolEvents.find(({ existing }) => existing.tool?.call_id === tool.call_id)?.index
    ?? (tool.cached === true && tool.cached_from
      ? toolEvents.find(({ existing }) => existing.tool?.call_id === tool.cached_from)?.index
      : undefined);
  if (idx != null && idx >= 0) {
    const prev = out[idx].tool;
    out[idx] = {
      ...out[idx],
      ...event,
      id: prev?.call_id ?? event.id,
      tool: prev ? mergeToolCall(prev, tool) : tool,
    };
  } else {
    out.push(event);
  }
}

function ArtifactEventCard(props: { event: ArtifactEventView; callbacks?: ArtifactCallbacks }) {
  switch (props.event.kind) {
    case "narration":
      return props.event.narration
        ? <NarrationArtifact id={props.event.id} steps={[props.event.narration]} />
        : null;
    case "narration_group":
      return props.event.narrations?.length
        ? <NarrationArtifact id={props.event.id} steps={props.event.narrations} />
        : null;
    case "search":
      return props.event.search ? <SearchArtifact search={props.event.search} /> : null;
    case "tool":
      return props.event.tool
        ? <ToolArtifact tool={props.event.tool} callbacks={props.callbacks} />
        : null;
    case "decision":
      return props.event.decision ? <DecisionArtifact id={props.event.id} draft={props.event.decision} /> : null;
  }
}

function NarrationArtifact(props: { id: string; steps: NarrationStep[] }) {
  return (
    <div
      data-call-id={props.id}
      class="lk-artifact-card is-wide is-reasoning"
      style={{
        background: "linear-gradient(180deg, rgba(217,119,87,0.08), rgba(255,255,255,0.02))",
        border: "1px solid rgba(217,119,87,0.24)",
        "border-radius": "8px",
        padding: "14px 16px",
      }}
    >
      <div style={{
        display: "flex",
        "align-items": "center",
        gap: "10px",
        "margin-bottom": "12px",
      }}>
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "11.5px",
          color: "var(--clay-soft)",
          "font-weight": 700,
          "text-transform": "uppercase",
          "letter-spacing": "0.08em",
        }}>
          思维轨迹
        </span>
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "11px",
          color: "var(--ink-3)",
        }}>
          {props.steps.length} step{props.steps.length > 1 ? "s" : ""}
        </span>
      </div>
      <div style={{ display: "flex", "flex-direction": "column", gap: "12px" }}>
        <For each={props.steps}>{(step, index) => (
          <div style={{
            display: "grid",
            "grid-template-columns": "28px minmax(0, 1fr)",
            gap: "10px",
            "align-items": "start",
            "padding-top": index() > 0 ? "10px" : "0",
            "border-top": index() > 0 ? "1px solid rgba(217,119,87,0.14)" : "0",
          }}>
            <div style={{
              width: "24px",
              height: "24px",
              "border-radius": "999px",
              display: "grid",
              "place-items": "center",
              background: "rgba(217,119,87,0.14)",
              color: "var(--clay-soft)",
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              "font-weight": 700,
            }}>
              {String(step.seq ?? index() + 1).padStart(2, "0")}
            </div>
            <div style={{
              color: "var(--ink-1)",
              "font-size": "13.5px",
              "line-height": 1.55,
            }}>
              <SafeMarkdown source={step.text} />
            </div>
          </div>
        )}</For>
      </div>
    </div>
  );
}

function DecisionArtifact(props: { id: string; draft: DecisionDraftView }) {
  const directionLabel = () =>
    props.draft.direction === "long" ? "BUY"
    : props.draft.direction === "short" ? "SELL"
    : props.draft.direction === "flat" ? "EXIT"
    : props.draft.direction.toUpperCase();
  const directionTone = () =>
    props.draft.direction === "long" ? "var(--up)"
    : props.draft.direction === "short" ? "var(--down)"
    : "var(--flat)";
  const stat = (label: string, value: string | number | undefined) => (
    <Show when={value != null && value !== ""}>
      <div style={{
        padding: "7px 8px",
        background: "rgba(255,255,255,0.03)",
        border: "1px solid rgba(255,255,255,0.08)",
        "border-radius": "6px",
        "min-width": 0,
      }}>
        <div style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)", "margin-bottom": "3px" }}>
          {label}
        </div>
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "13px",
          color: "var(--ink-0)",
          "font-weight": 700,
          overflow: "hidden",
          "text-overflow": "ellipsis",
          "white-space": "nowrap",
        }}>
          {value}
        </div>
      </div>
    </Show>
  );

  return (
    <div
      data-call-id={props.id}
      class="lk-artifact-card is-normal is-action"
      style={{
        background: "linear-gradient(180deg, rgba(232,184,108,0.10), rgba(255,255,255,0.02))",
        border: "1px solid rgba(232,184,108,0.28)",
        "border-radius": "8px",
        padding: "14px 15px",
      }}
    >
      <div style={{
        display: "flex",
        "align-items": "baseline",
        gap: "10px",
        "font-family": "var(--font-mono)",
        "margin-bottom": "10px",
      }}>
        <span style={{ color: directionTone(), "font-size": "13px", "font-weight": 800, "letter-spacing": "0.08em" }}>
          ACTION · {directionLabel()}
        </span>
        <span style={{ color: "var(--ink-2)", "font-size": "13px", "font-weight": 700 }}>
          {props.draft.ticker}
        </span>
      </div>
      <div style={{
        display: "grid",
        "grid-template-columns": "repeat(2, minmax(0, 1fr))",
        gap: "8px",
        "margin-bottom": "10px",
      }}>
        {stat("Size", props.draft.size_pct != null ? `${props.draft.size_pct}%` : undefined)}
        {stat("Horizon", props.draft.horizon_days != null ? `${props.draft.horizon_days}d` : undefined)}
        {stat("Stop", props.draft.stop_loss)}
        {stat("Target", props.draft.target)}
      </div>
      <div style={{
        color: "var(--ink-1)",
        "font-size": "13.5px",
        "line-height": 1.55,
        display: "-webkit-box",
        "-webkit-line-clamp": "5",
        "-webkit-box-orient": "vertical",
        overflow: "hidden",
      }}>
        {props.draft.rationale}
      </div>
    </div>
  );
}

function StatusDot(props: { status: string }) {
  const color = () =>
    props.status === "completed" ? "#9eb78f" :
    props.status === "error" ? "#d97070" : "#d9a76c";
  return (
    <span style={{
      width: "6px",
      height: "6px",
      "border-radius": "50%",
      background: color(),
      display: "inline-block",
      "margin-right": "8px",
      "flex-shrink": 0,
    }} />
  );
}

function ToolStatusBadges(props: { tool?: ToolCallView }) {
  const cachedLabel = () => props.tool?.cached ? "cached" : "";
  const partialLabel = () => {
    switch (props.tool?.partial_status) {
      case "partial_with_unavailable_source": return "部分来源不可用";
      case "valid_empty": return "无匹配数据";
      case "success_with_no_data": return "无数据";
      default: return props.tool?.partial_status ?? "";
    }
  };
  const errorLabel = () => {
    if (!props.tool?.error_kind) return "";
    return props.tool.retryable === true
      ? `${props.tool.error_kind} · retryable`
      : props.tool.retryable === false
      ? `${props.tool.error_kind} · final`
      : props.tool.error_kind;
  };
  return (
    <Show when={cachedLabel() || partialLabel() || errorLabel()}>
      <span style={{
        display: "inline-flex",
        "align-items": "center",
        gap: "5px",
        "margin-left": "8px",
        "min-width": 0,
      }}>
        <Show when={cachedLabel()}>
          <span
            title={props.tool?.cached_from ? `cached from ${props.tool.cached_from}` : "cached result"}
            style={{
              color: "rgba(158,183,143,0.95)",
              background: "rgba(158,183,143,0.10)",
              border: "1px solid rgba(158,183,143,0.18)",
              "border-radius": "999px",
              padding: "1px 6px",
              "font-size": "10.5px",
              "line-height": 1.45,
              "text-transform": "uppercase",
              "letter-spacing": "0.04em",
            }}
          >
            cached
          </span>
        </Show>
        <Show when={errorLabel()}>
          <span style={{
            color: "rgba(217,112,112,0.95)",
            background: "rgba(217,112,112,0.10)",
            border: "1px solid rgba(217,112,112,0.18)",
            "border-radius": "999px",
            padding: "1px 6px",
            "font-size": "10.5px",
            "line-height": 1.45,
            "text-transform": "uppercase",
            "letter-spacing": "0.04em",
            overflow: "hidden",
            "text-overflow": "ellipsis",
            "white-space": "nowrap",
          }}>
            {errorLabel()}
          </span>
        </Show>
        <Show when={partialLabel()}>
          <span style={{
            color: "rgba(217,167,108,0.98)",
            background: "rgba(217,167,108,0.10)",
            border: "1px solid rgba(217,167,108,0.18)",
            "border-radius": "999px",
            padding: "1px 6px",
            "font-size": "10.5px",
            "line-height": 1.45,
            "text-transform": "uppercase",
            "letter-spacing": "0.04em",
            overflow: "hidden",
            "text-overflow": "ellipsis",
            "white-space": "nowrap",
          }}>
            {partialLabel()}
          </span>
        </Show>
      </span>
    </Show>
  );
}

function CardShell(props: {
  status: string;
  badge: string;
  detail?: string;
  children: any;
  onClick?: () => void;
  tool?: ToolCallView;
  /** Marks the card so chat-side click handlers can scrollIntoView. */
  callId?: string;
  /** How many grid columns this card spans (default 1). */
  span?: number;
  size?: "compact" | "normal" | "wide" | "hero";
}) {
  return (
    <div
      onClick={props.onClick}
      data-call-id={props.callId}
      class={`lk-artifact-card is-${props.size ?? "normal"}${props.onClick ? " is-clickable" : ""}`}
      style={{
        background: "var(--bg-1)",
        border: "1px solid var(--bg-2)",
        "border-radius": "8px",
        padding: "12px 14px",
        cursor: props.onClick ? "pointer" : "default",
        transition: "border-color 120ms, box-shadow 200ms",
      }}
    >
      <div style={{
        display: "flex",
        "align-items": "center",
        "flex-wrap": "wrap",
        gap: "0",
        "row-gap": "5px",
        "margin-bottom": "8px",
        "font-family": "var(--font-mono)",
        "font-size": "11.5px",
        color: "var(--ink-3)",
        "text-transform": "uppercase",
        "letter-spacing": "0.04em",
      }}>
        <StatusDot status={props.status} />
        <span style={{ color: "var(--ink-2)", "font-weight": 600 }}>{props.badge}</span>
        <Show when={props.detail}>
          <span style={{
            "margin-left": "10px",
            "text-transform": "none",
            "letter-spacing": "0",
            overflow: "hidden",
            "text-overflow": "ellipsis",
            "white-space": "nowrap",
            "min-width": 0,
            "max-width": "100%",
          }}>
            {props.detail}
          </span>
        </Show>
        <ToolStatusBadges tool={props.tool} />
      </div>
      {props.children}
    </div>
  );
}

function ArtifactModal(props: {
  title: string;
  subtitle?: string;
  onClose: () => void;
  children: any;
}) {
  let dialogEl: HTMLDivElement | undefined;
  // Esc closes; focus moves into the dialog on open and is restored to the
  // trigger on close (WCAG 2.4.3 / 2.1.2 — a modal is a deliberate focus trap
  // that releases on Escape, not a violation).
  onMount(() => {
    const prevFocus = document.activeElement as HTMLElement | null;
    dialogEl?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        props.onClose();
      }
    };
    document.addEventListener("keydown", onKey);
    onCleanup(() => {
      document.removeEventListener("keydown", onKey);
      prevFocus?.focus?.();
    });
  });
  return (
    <Portal>
      <div
        onClick={props.onClose}
        style={{
          position: "fixed",
          inset: "0",
          background: "rgba(0,0,0,0.62)",
          "backdrop-filter": "blur(8px)",
          "z-index": 60,
          display: "grid",
          "place-items": "center",
          padding: "28px",
        }}
      >
        <div
          ref={(el) => (dialogEl = el)}
          role="dialog"
          aria-modal="true"
          aria-label={props.title}
          tabindex="-1"
          onClick={(e) => e.stopPropagation()}
          style={{
            width: "min(1320px, 96vw)",
            height: "min(900px, 92vh)",
            background: "var(--bg-1)",
            border: "1px solid var(--line-2)",
            "border-radius": "var(--r-4)",
            overflow: "hidden",
            display: "flex",
            "flex-direction": "column",
            "box-shadow": "0 30px 90px rgba(0,0,0,0.55)",
            outline: "none",
          }}
        >
          <div style={{
            padding: "18px 22px 14px",
            display: "flex",
            "align-items": "start",
            gap: "14px",
            "border-bottom": "1px solid rgba(255,255,255,0.08)",
          }}>
            <div style={{ "min-width": 0 }}>
              <div style={{ "font-size": "13px", color: "var(--ink-3)", "font-family": "var(--font-mono)", "margin-bottom": "3px" }}>
                {props.subtitle}
              </div>
              <div style={{ "font-size": "20px", color: "var(--ink-0)", "font-weight": 650, "line-height": 1.15 }}>
                {props.title}
              </div>
            </div>
            <button
              type="button"
              onClick={props.onClose}
              style={{
                "margin-left": "auto",
                width: "30px",
                height: "30px",
                display: "grid",
                "place-items": "center",
                appearance: "none",
                background: "rgba(255,255,255,0.06)",
                border: "1px solid rgba(255,255,255,0.12)",
                color: "var(--ink-2)",
                "border-radius": "6px",
                cursor: "pointer",
                "font-size": "18px",
                "line-height": 1,
              }}
              aria-label="Close"
            >
              ×
            </button>
          </div>
          <div style={{ overflow: "auto", padding: "22px" }}>
            {props.children}
          </div>
        </div>
      </div>
    </Portal>
  );
}

function SearchArtifact(props: { search: SearchCallView }) {
  const badge = () =>
    props.search.action === "open_page" ? "网页 · 打开" :
    props.search.action === "find_in_page" ? "网页 · 页内查找" :
    "网页 · 搜索";
  const verb = () => props.search.status === "completed"
    ? props.search.action === "open_page" ? "已打开"
    : props.search.action === "find_in_page" ? "已查找"
    : "已搜索"
    : props.search.action === "open_page" ? "正在打开"
    : props.search.action === "find_in_page" ? "正在查找"
    : "正在搜索";
  const primaryDetail = () => props.search.detail || props.search.queries?.[0] || "";
  let host = primaryDetail();
  let isUrl = false;
  try {
    const u = new URL(primaryDetail());
    host = u.hostname.replace(/^www\./, "");
    isUrl = true;
  } catch {/* leave as is */}

  const onClick = () => {
    if (isUrl) window.open(primaryDetail(), "_blank", "noopener,noreferrer");
  };
  const queryBatch = () => (props.search.queries ?? []).filter(Boolean);
  const sources = () => (props.search.sources ?? []).filter(Boolean);
  const sourceHost = (url: string) => {
    try { return new URL(url).hostname.replace(/^www\./, ""); }
    catch { return url; }
  };

  return (
    <div
      data-call-id={searchKey(props.search)}
      class="lk-artifact-card is-compact"
      onClick={isUrl ? onClick : undefined}
      style={{
        background: "var(--bg-1)",
        border: "1px solid var(--bg-2)",
        "border-radius": "8px",
        padding: "12px 14px",
        cursor: isUrl ? "pointer" : "default",
        transition: "border-color 120ms, box-shadow 200ms",
        display: "flex",
        "flex-direction": "column",
        gap: "6px",
      }}
    >
      <div style={{
        display: "flex",
        "align-items": "center",
        "font-family": "var(--font-mono)",
        "font-size": "11.5px",
        color: "var(--ink-3)",
        "text-transform": "uppercase",
        "letter-spacing": "0.04em",
      }}>
        <StatusDot status={props.search.status} />
        <span style={{ color: "var(--ink-2)", "font-weight": 600 }}>{badge()}</span>
        <span style={{ "margin-left": "10px", "text-transform": "none", "letter-spacing": "0" }}>
          {verb()}
        </span>
      </div>
      <div style={{
        "font-size": "13px",
        color: isUrl ? "var(--ink-1)" : "var(--ink-2)",
        "word-break": "break-word",
        "line-height": 1.4,
      }}>
        {isUrl ? host : (primaryDetail() || "—")}
      </div>
      <Show when={props.search.action === "search" && queryBatch().length > 1}>
        <div style={{
          display: "flex",
          "flex-direction": "column",
          gap: "5px",
          "margin-top": "2px",
        }}>
          <For each={queryBatch().slice(0, 4)}>{(q) => (
            <div style={{
              "font-family": "var(--font-mono)",
              "font-size": "11.5px",
              color: "var(--ink-2)",
              padding: "4px 6px",
              background: "rgba(255,255,255,0.03)",
              border: "1px solid var(--line-1)",
              "border-radius": "4px",
              overflow: "hidden",
              "text-overflow": "ellipsis",
              "white-space": "nowrap",
            }}>{q}</div>
          )}</For>
        </div>
      </Show>
      <Show when={sources().length > 0}>
        <div style={{
          display: "flex",
          "flex-direction": "column",
          gap: "5px",
          "margin-top": "2px",
        }}>
          <div style={{
            "font-family": "var(--font-mono)",
            "font-size": "11px",
            color: "var(--ink-3)",
            "text-transform": "uppercase",
            "letter-spacing": "0",
          }}>sources</div>
          <For each={sources().slice(0, 5)}>{(url) => (
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                window.open(url, "_blank", "noopener,noreferrer");
              }}
              style={{
                appearance: "none",
                background: "rgba(255,255,255,0.03)",
                border: "1px solid var(--line-1)",
                color: "var(--ink-2)",
                "font-family": "var(--font-mono)",
                "font-size": "11.5px",
                padding: "4px 6px",
                "border-radius": "4px",
                overflow: "hidden",
                "text-overflow": "ellipsis",
                "white-space": "nowrap",
                cursor: "pointer",
                "text-align": "left",
              }}
            >{sourceHost(url)}</button>
          )}</For>
        </div>
      </Show>
      <Show when={isUrl}>
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "11.5px",
          color: "var(--ink-3)",
          "white-space": "nowrap",
          overflow: "hidden",
          "text-overflow": "ellipsis",
        }}>{primaryDetail()}</div>
      </Show>
    </div>
  );
}

function ToolArtifact(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  switch (props.tool.name) {
    case "corpus_search": return <CorpusSearchCard {...props} />;
    case "corpus_read": return <CorpusReadCard {...props} />;
    case "web_fetch":     return <WebFetchCard {...props} />;
    case "market_quote":
    case "tradingview_quote": return <MarketQuoteCard {...props} />;
    case "get_a_share_market_snapshot": return <AShareMarketSnapshotCard {...props} />;
    case "get_candlesticks": return <CandlestickToolCard {...props} />;
    case "get_company_info": return <CompanyInfoCard {...props} />;
    case "get_financials": return <FinancialsCard {...props} />;
    case "get_a_share_industry_context": return <AShareIndustryContextCards {...props} />;
    case "get_a_share_research_sources": return <AShareResearchSourcesCard {...props} />;
    case "get_capital_flow": return <CapitalFlowCard {...props} />;
    case "get_a_share_ownership_and_leverage": return <MarkdownContextCards {...props} badge={toolDisplayName(props.tool.name)} />;
    case "get_china_macro_context": return <MarkdownContextCards {...props} badge={toolDisplayName(props.tool.name)} />;
    case "get_china_fund_context": return <MarkdownContextCards {...props} badge={toolDisplayName(props.tool.name)} />;
    case "get_china_index_context": return <MarkdownContextCards {...props} badge={toolDisplayName(props.tool.name)} />;
    case "get_crypto_market": return <MarkdownContextCards {...props} badge={toolDisplayName(props.tool.name)} />;
    case "get_funding_rate": return <MarkdownContextCards {...props} badge={toolDisplayName(props.tool.name)} />;
    case "sec_filing_fetch": return <SecFilingCard {...props} />;
    default:              return <GenericToolCard {...props} />;
  }
}

type ToolDisplayLocale = "zh" | "en";
interface ToolDisplayName {
  zh: string;
  en: string;
}

const TOOL_DISPLAY_NAMES: Record<string, ToolDisplayName> = {
  ask_user_question: { zh: "代理 · 追问", en: "Agent Question" },
  corpus_search: { zh: "投研智库 · 检索", en: "Investment Corpus Search" },
  corpus_read: { zh: "投研智库 · 阅读", en: "Investment Corpus Read" },
  delegate_research: { zh: "研究 · 委派", en: "Delegated Research" },
  get_a_share_industry_context: { zh: "A股 · 行业上下文", en: "A-share Industry Context" },
  get_a_share_market_snapshot: { zh: "A股 · 行情快照", en: "A-share Market Snapshot" },
  get_a_share_ownership_and_leverage: { zh: "A股 · 筹码杠杆", en: "A-share Ownership & Leverage" },
  get_a_share_research_sources: { zh: "A股 · 源材料", en: "A-share Source Materials" },
  get_candlesticks: { zh: "市场 · K线走势", en: "Market Candlesticks" },
  get_capital_flow: { zh: "A股 · 资金流", en: "A-share Capital Flow" },
  get_china_fund_context: { zh: "中国 · 基金上下文", en: "China Fund Context" },
  get_china_index_context: { zh: "中国 · 指数上下文", en: "China Index Context" },
  get_china_macro_context: { zh: "中国 · 宏观上下文", en: "China Macro Context" },
  get_company_info: { zh: "公司 · 基本资料", en: "Company Profile" },
  get_crypto_market: { zh: "加密 · 市场上下文", en: "Crypto Market Context" },
  get_financials: { zh: "财务 · 三表指标", en: "Financial Statements & Ratios" },
  get_funding_rate: { zh: "加密 · 合约资金面", en: "Funding & Open Interest" },
  market_quote: { zh: "市场 · 报价", en: "Market Quote" },
  sec_filing_fetch: { zh: "SEC · 文件", en: "SEC Filings" },
  tradingview_quote: { zh: "市场 · 报价", en: "Market Quote" },
  update_plan: { zh: "计划 · 更新", en: "Plan Update" },
  web_fetch: { zh: "网页 · 读取", en: "Web Fetch" },
};

export function toolDisplayName(name: string, locale: ToolDisplayLocale = "zh"): string {
  return TOOL_DISPLAY_NAMES[name]?.[locale] ?? name.replace(/_/g, " · ");
}

function safeParseJson(s: string | undefined): unknown {
  if (!s) return null;
  try { return JSON.parse(s); }
  catch {
    // Truncated JSON — try shaving the tail until we get a parseable prefix.
    for (let i = s.length - 1; i > 32; i--) {
      const tail = s.slice(0, i);
      try { return JSON.parse(tail); } catch {/* keep shaving */}
    }
    return null;
  }
}

/** Pull complete `{...}` objects out of a JSON array literal whose tail is
 *  truncated. Used to salvage partial corpus_search / sec_filing_fetch outputs
 *  where the preview cap cuts inside the last entry. */
function extractObjectsAfter(s: string, marker: string): unknown[] {
  const startIdx = s.indexOf(marker);
  if (startIdx < 0) return [];
  let i = startIdx + marker.length;
  // Skip to the `[`
  while (i < s.length && s[i] !== "[") i++;
  if (s[i] !== "[") return [];
  i++;
  const out: unknown[] = [];
  while (i < s.length) {
    while (i < s.length && /[\s,]/.test(s[i])) i++;
    if (s[i] !== "{") break;
    const start = i;
    let depth = 0;
    let inStr = false;
    let esc = false;
    let end = -1;
    for (; i < s.length; i++) {
      const c = s[i];
      if (esc) { esc = false; continue; }
      if (c === "\\") { esc = true; continue; }
      if (c === '"') { inStr = !inStr; continue; }
      if (inStr) continue;
      if (c === "{") depth++;
      else if (c === "}") {
        depth--;
        if (depth === 0) { end = i; i++; break; }
      }
    }
    if (end < 0) break;
    try { out.push(JSON.parse(s.slice(start, end + 1))); }
    catch {/* skip malformed entry */}
  }
  return out;
}

function parseArgs(t: ToolCallView): Record<string, unknown> {
  if (!t.arguments) return {};
  try { return JSON.parse(t.arguments); }
  catch { return {}; }
}

function toolOutput(tool: ToolCallView): string {
  const artifact = tool.ui_artifact;
  const payload = artifact?.payload;
  if (artifact?.content_type === "markdown" && isRecord(payload) && typeof payload.markdown === "string") {
    return payload.markdown;
  }
  if (artifact?.content_type === "text" && isRecord(payload) && typeof payload.text === "string") {
    return payload.text;
  }
  if (artifact?.content_type === "json" && payload !== undefined && payload !== null) {
    try { return JSON.stringify(payload); } catch { /* fall back to preview */ }
  }
  return tool.output_preview ?? "";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function fetchToolPreview(url: URL): Promise<string> {
  const key = url.toString();
  const existing = toolPreviewCache.get(key);
  if (existing) return existing;
  const pending = fetch(url)
    .then(async (r) => {
      const text = await r.text();
      let body: any;
      try {
        body = JSON.parse(text);
      } catch {
        throw new Error("工具数据接口返回非 JSON 响应");
      }
      if (!r.ok) throw new Error(body?.error?.message || "工具数据接口请求失败");
      return String(body.output_preview ?? "");
    })
    .catch((err) => {
      toolPreviewCache.delete(key);
      throw err;
    });
  toolPreviewCache.set(key, pending);
  return pending;
}

const toolPreviewCache = new Map<string, Promise<string>>();

function firstHeading(md: string): string {
  return md.match(/^##?\s+(.+)$/m)?.[1]?.trim() ?? "";
}

function securityLabel(text: string): string {
  const heading = text.match(/^##\s+(.+)$/m)?.[1];
  if (!heading) return "";
  return heading.split(" · ")[0]?.trim() ?? "";
}

interface MarkdownTable {
  title: string;
  headers: string[];
  rows: string[][];
}

function parseMarkdownTables(md: string): MarkdownTable[] {
  const out: MarkdownTable[] = [];
  const lines = md.split("\n");
  let title = "";
  for (let i = 0; i < lines.length; i++) {
    const heading = lines[i].match(/^#{2,3}\s+(.+)$/);
    if (heading) {
      title = heading[1].trim();
      continue;
    }
    if (!lines[i].startsWith("|") || !lines[i + 1]?.includes("---")) continue;
    const headers = splitMarkdownRow(lines[i]);
    i += 2;
    const rows: string[][] = [];
    while (i < lines.length && lines[i].startsWith("|")) {
      rows.push(splitMarkdownRow(lines[i]));
      i++;
    }
    i--;
    out.push({ title, headers, rows });
  }
  return out;
}

function splitMarkdownRow(line: string): string[] {
  return line
    .split("|")
    .map((c) => c.trim())
    .filter((_, i, arr) => i > 0 && i < arr.length - 1);
}

/* ---------- corpus_search ---------- */

interface CorpusHit {
  id: string;
  title: string;
  tier: string;
  layer: string;
  tags?: string[];
  score?: number;
  snippet?: string;
}

interface CorpusReadDoc {
  id: string;
  title: string;
  tier: string;
  layer: string;
  tags: string[];
  body: string;
}

function parseCorpusRead(md: string, fallbackId: string): CorpusReadDoc {
  const title = firstHeading(md);
  const readLine = (field: string) => {
    const match = md.match(new RegExp(`^- ${field}:\\s*(.+)$`, "m"));
    return match?.[1]?.replace(/^`|`$/g, "").trim() ?? "";
  };
  const tagsRaw = readLine("tags");
  const body = md
    .replace(/^# .+\n\n(?:- .+\n)+\n/, "")
    .replace(/\n\n\[corpus_read:[\s\S]*$/m, "")
    .trim();
  return {
    id: readLine("id") || fallbackId,
    title: title || fallbackId,
    tier: readLine("tier"),
    layer: readLine("layer"),
    tags: !tagsRaw || tagsRaw === "(none)" ? [] : tagsRaw.split(",").map((tag) => tag.trim()).filter(Boolean),
    body,
  };
}

function CorpusSearchCard(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  const args = parseArgs(props.tool);
  const query = String(args.query ?? "");
  const preview = toolOutput(props.tool);
  const parsed = safeParseJson(preview) as
    | { hits?: CorpusHit[]; total_corpus_docs?: number } | null;
  // Fall back to brace-balanced extraction when the preview was truncated
  // mid-record (preview cap < full output).
  const hits: CorpusHit[] = parsed?.hits
    ?? (extractObjectsAfter(preview, "\"hits\"") as CorpusHit[]);
  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={query ? `"${query}"` : undefined}
      tool={props.tool}
      callId={props.tool.call_id}
      size="normal"
    >
      <Show
        when={hits.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No hits.">
              Searching the corpus…
            </Show>
          </div>
        }
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
          <For each={hits.slice(0, 5)}>{(h) => (
            <CorpusHitTile hit={h} onOpen={() =>
              props.callbacks?.onOpenDoc?.(h.id, h.title || h.id)
            } />
          )}</For>
        </div>
      </Show>
    </CardShell>
  );
}

function CorpusReadCard(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  const args = parseArgs(props.tool);
  const fallbackId = String(args.id ?? "");
  const doc = createMemo(() => parseCorpusRead(toolOutput(props.tool), fallbackId));
  const openDoc = () => {
    if (doc().id) props.callbacks?.onOpenDoc?.(doc().id, doc().title || doc().id);
  };

  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={doc().id || fallbackId}
      tool={props.tool}
      callId={props.tool.call_id}
      size="wide"
      onClick={doc().id ? openDoc : undefined}
    >
      <Show
        when={doc().title || doc().body}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="未读取到智库页面。">
              正在读取投研智库页面…
            </Show>
          </div>
        }
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "9px" }}>
          <div style={{ display: "flex", "align-items": "baseline", gap: "8px", "min-width": 0 }}>
            <div style={{
              "font-size": "15px",
              "font-weight": 650,
              color: "var(--ink-0)",
              overflow: "hidden",
              "text-overflow": "ellipsis",
              "white-space": "nowrap",
            }}>
              {doc().title}
            </div>
            <Show when={doc().tier || doc().layer}>
              <div style={{
                "font-family": "var(--font-mono)",
                "font-size": "11px",
                color: "var(--ink-3)",
                "white-space": "nowrap",
              }}>
                {[doc().tier, doc().layer].filter(Boolean).join(" · ")}
              </div>
            </Show>
          </div>
          <Show when={doc().tags.length > 0}>
            <div style={{ display: "flex", gap: "5px", "flex-wrap": "wrap" }}>
              <For each={doc().tags.slice(0, 6)}>{(tag) => (
                <span style={{
                  "font-family": "var(--font-mono)",
                  "font-size": "11px",
                  color: "var(--clay-soft)",
                  background: "rgba(217,119,87,0.08)",
                  border: "1px solid rgba(217,119,87,0.14)",
                  "border-radius": "999px",
                  padding: "2px 6px",
                }}>{tag}</span>
              )}</For>
            </div>
          </Show>
          <div
            onClick={(e) => e.stopPropagation()}
            style={{
            "max-height": "230px",
            overflow: "hidden",
            "font-size": "13px",
            color: "var(--ink-2)",
            "line-height": 1.55,
          }}>
            <SafeMarkdown source={doc().body} onWikiOpen={(id) => props.callbacks?.onOpenDoc?.(id, id)} />
          </div>
          <Show when={doc().id}>
            <div style={{ "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--clay-soft)" }}>
              点击查看完整智库页面
            </div>
          </Show>
        </div>
      </Show>
    </CardShell>
  );
}

function CorpusHitTile(props: { hit: CorpusHit; onOpen: () => void }) {
  const tierLabel = () => `${props.hit.tier}·${props.hit.layer}`;
  return (
    <div
      onClick={(e) => { e.stopPropagation(); props.onOpen(); }}
      title="Click to open the full wiki page"
      style={{
        cursor: "pointer",
        padding: "5px 8px",
        background: "rgba(255, 255, 255, 0.025)",
        border: "1px solid var(--line-1)",
        "border-radius": "4px",
        display: "flex",
        "align-items": "center",
        gap: "8px",
        "min-height": "24px",
      }}
    >
      <span style={{
        "font-size": "13px",
        color: "var(--ink-0)",
        flex: 1,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>{props.hit.title}</span>
      <span style={{
        "font-family": "var(--font-mono)",
        "font-size": "10.5px",
        color: "var(--ink-3)",
        "flex-shrink": 0,
      }}>{tierLabel()}</span>
    </div>
  );
}

/* ---------- web_fetch · link preview ---------- */

export interface LinkMeta {
  title: string;
  description: string;
  thumbnail?: string;
  unusable?: boolean;
}

function unusableFetchReason(md: string): string {
  const match = md.match(/\[web_fetch: clearly unusable \/ not citable \(([^)]+)\)\./i);
  if (!match) return "";
  switch (match[1]) {
    case "binary_skipped": return "该来源是二进制文件，当前无法在卡片中读取正文。";
    case "unreadable_or_garbled_text": return "该页面正文不可读或乱码，不能作为可引用证据。";
    default: return "该来源无法提取可引用正文。";
  }
}

/** Pull page title (first `# heading`), short description, and thumbnail
 *  (first markdown image) out of the truncated readability output. */
export function extractLinkMeta(md: string): LinkMeta {
  const unusableReason = unusableFetchReason(md);
  if (unusableReason) {
    return {
      title: "来源未读取",
      description: `${unusableReason} 已保留调试信息在事件记录中；结论应改用可读取的网页、公告文本或结构化工具结果。`,
      unusable: true,
    };
  }

  // The fetched markdown is preceded by "# Source: <url>\n_(extraction: ...)_\n\n"
  // — strip both lines first.
  const body = md
    .replace(/^# Source:[^\n]*\n/, "")
    .replace(/^_\(extraction:[^\n]*\)_\n\n/m, "")
    .trim();

  const titleMatch = body.match(/^#{1,3}\s+(.+)$/m);
  const title = titleMatch ? titleMatch[1].trim() : "";

  // Description: text after the title, first non-empty paragraph, plain-text-ified.
  const afterTitle = title
    ? body.slice((titleMatch?.index ?? 0) + (titleMatch?.[0].length ?? 0)).trim()
    : body;
  // Skip image-only paragraphs and metadata lines (italic single-line).
  const paragraphs = afterTitle.split(/\n\n+/);
  let desc = "";
  for (const p of paragraphs) {
    const stripped = p
      .replace(/!\[[^\]]*\]\([^)]+\)/g, "")
      .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
      .replace(/[*_`>#]+/g, "")
      .replace(/\s+/g, " ")
      .trim();
    if (stripped.length >= 20) {
      desc = stripped;
      break;
    }
  }
  if (desc.length > 160) desc = desc.slice(0, 157).trimEnd() + "…";

  const imgMatch = body.match(/!\[[^\]]*\]\((https?:\/\/[^)\s]+)\)/);
  const thumbnail = imgMatch?.[1];

  return { title, description: desc, thumbnail };
}

function WebFetchCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const url = String(args.url ?? "");
  let host = url;
  try { host = new URL(url).hostname.replace(/^www\./, ""); } catch {/* leave */}
  const favicon = host
    ? `https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=64`
    : "";

  const meta = createMemo(() => extractLinkMeta(toolOutput(props.tool)));

  return (
    <div
      onClick={url ? () => window.open(url, "_blank", "noopener,noreferrer") : undefined}
      data-call-id={props.tool.call_id}
      class="lk-artifact-card is-normal"
      style={{
        background: meta().unusable ? "rgba(217,112,112,0.06)" : "var(--bg-1)",
        border: meta().unusable ? "1px solid rgba(217,112,112,0.26)" : "1px solid var(--bg-2)",
        "border-radius": "8px",
        cursor: url ? "pointer" : "default",
        transition: "border-color 120ms, box-shadow 200ms",
        overflow: "hidden",
        display: "flex",
        "flex-direction": "column",
      }}
    >
      <Show when={meta().thumbnail}>
        <div style={{
          width: "100%",
          height: "120px",
          "background-image": `url(${meta().thumbnail})`,
          "background-size": "cover",
          "background-position": "center",
          "background-color": "var(--bg-2)",
          "border-bottom": "1px solid var(--bg-2)",
        }} />
      </Show>
      <div style={{ padding: "12px 14px", display: "flex", "flex-direction": "column", gap: "6px" }}>
        <div style={{
          display: "flex",
          "align-items": "center",
          gap: "6px",
          "font-family": "var(--font-mono)",
          "font-size": "11.5px",
          color: "var(--ink-3)",
          "text-transform": "uppercase",
          "letter-spacing": "0.04em",
        }}>
          <StatusDot status={props.tool.status} />
          <Show when={favicon}>
            <img src={favicon} width="14" height="14" alt="" style={{
              "border-radius": "3px",
              "flex-shrink": 0,
            }} />
          </Show>
          <span style={{
            "text-transform": "none",
            "letter-spacing": "0",
            overflow: "hidden",
            "text-overflow": "ellipsis",
            "white-space": "nowrap",
          }}>{host}</span>
          <ToolStatusBadges tool={props.tool} />
        </div>
        <Show
          when={meta().title || meta().description}
          fallback={
            <div style={{ color: "var(--ink-3)", "font-size": "13px" }}>
              <Show when={props.tool.status === "in_progress"} fallback="No content.">
                Fetching…
              </Show>
            </div>
          }
        >
          <Show when={meta().title}>
            <div style={{
              "font-size": "13.5px",
              "font-weight": 600,
              color: "var(--ink-0)",
              "line-height": 1.35,
              display: "-webkit-box",
              "-webkit-line-clamp": "2",
              "-webkit-box-orient": "vertical",
              overflow: "hidden",
            }}>{meta().title}</div>
          </Show>
          <Show when={meta().description}>
            <div style={{
              "font-size": "13px",
              color: "var(--ink-2)",
              "line-height": 1.5,
              display: "-webkit-box",
              "-webkit-line-clamp": "3",
              "-webkit-box-orient": "vertical",
              overflow: "hidden",
            }}>{meta().description}</div>
          </Show>
        </Show>
      </div>
    </div>
  );
}

/* ---------- get_company_info ---------- */

interface CompanyInfo {
  title: string;
  basics: Array<[string, string]>;
  intro: string;
  metrics: Array<[string, string]>;
}

function parseCompanyInfo(md: string): CompanyInfo {
  const title = firstHeading(md);
  const basics: Array<[string, string]> = [];
  const metrics: Array<[string, string]> = [];
  let intro = "";
  const lines = md.split("\n");
  let section = "";
  for (const line of lines) {
    if (/^\*\*基本信息\*\*/.test(line)) {
      section = "basic";
      continue;
    }
    if (/^\*\*公司简介\*\*/.test(line)) {
      section = "intro";
      continue;
    }
    if (/^\*\*最新财务指标\*\*/.test(line)) {
      section = "metrics";
      continue;
    }
    if (section === "basic") {
      const m = line.match(/^-\s*([^：:]+)[：:]\s*(.+)$/);
      if (m) basics.push([m[1].trim(), m[2].trim()]);
    } else if (section === "intro") {
      if (line.trim() && !line.startsWith("_Source")) intro += `${line.trim()}\n`;
    } else if (section === "metrics" && line.startsWith("|") && !line.includes("---") && !line.includes("指标")) {
      const cells = splitMarkdownRow(line);
      if (cells.length >= 2) metrics.push([cells[0], cells[1]]);
    }
  }
  return { title, basics, intro: intro.trim(), metrics };
}

function CompanyInfoCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tsCode = String(args.ts_code ?? "");
  const [open, setOpen] = createSignal(false);
  const info = createMemo(() => parseCompanyInfo(toolOutput(props.tool)));
  const selectedBasics = () => info().basics.filter(([k]) =>
    ["所在地", "成立日期", "员工数"].includes(k)
  );
  const mainBusiness = () => info().basics.find(([k]) => k === "主营业务")?.[1] ?? "";
  const selectedMetrics = () => info().metrics.filter(([k]) =>
    ["ROE", "毛利率", "资产负债率", "PE(TTM)", "PB", "营收同比", "净利润同比"].includes(k)
  );

  return (
    <>
      <CardShell
        status={props.tool.status}
        badge={toolDisplayName(props.tool.name)}
        detail={info().title || tsCode}
        tool={props.tool}
        callId={props.tool.call_id}
        size="wide"
        onClick={() => setOpen(true)}
      >
        <Show
          when={info().title || selectedBasics().length || selectedMetrics().length}
          fallback={
            <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
              <Show when={props.tool.status === "in_progress"} fallback="No company profile.">
                Fetching company profile…
              </Show>
            </div>
          }
        >
          <div style={{
            display: "grid",
            "grid-template-columns": selectedMetrics().length > 0 ? "minmax(0, 1.1fr) minmax(180px, 0.9fr)" : "minmax(0, 1fr)",
            gap: "14px",
          }}>
            <div style={{ display: "flex", "flex-direction": "column", gap: "8px", "min-width": 0 }}>
              <div style={{ "font-size": "15px", "font-weight": 650, color: "var(--ink-0)", "line-height": 1.25 }}>
                {info().title}
              </div>
              <div style={{ display: "grid", "grid-template-columns": "repeat(3, minmax(0, 1fr))", gap: "6px" }}>
                <For each={selectedBasics().slice(0, 3)}>{([k, v]) => (
                  <div style={{
                    padding: "6px 8px",
                    background: "rgba(255,255,255,0.025)",
                    border: "1px solid var(--line-1)",
                    "border-radius": "6px",
                    "min-width": 0,
                  }}>
                    <div style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)" }}>{k}</div>
                    <div style={{ "font-size": "12.5px", color: "var(--ink-1)", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>{v}</div>
                  </div>
                )}</For>
              </div>
              <Show when={mainBusiness()}>
                <div style={{
                  padding: "8px 9px",
                  background: "rgba(255,255,255,0.025)",
                  border: "1px solid var(--line-1)",
                  "border-radius": "6px",
                  color: "var(--ink-2)",
                  "font-size": "13px",
                  "line-height": 1.5,
                  display: "-webkit-box",
                  "-webkit-line-clamp": "2",
                  "-webkit-box-orient": "vertical",
                  overflow: "hidden",
                }}>{mainBusiness()}</div>
              </Show>
              <Show when={info().intro}>
                <div style={{
                  "font-size": "13px",
                  color: "var(--ink-2)",
                  "line-height": 1.55,
                  display: "-webkit-box",
                  "-webkit-line-clamp": mainBusiness() ? "2" : "3",
                  "-webkit-box-orient": "vertical",
                  overflow: "hidden",
                }}>{info().intro}</div>
              </Show>
              <div style={{ "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--clay-soft)" }}>
                点击查看完整公司资料
              </div>
            </div>
            <Show when={selectedMetrics().length > 0}>
              <div style={{
                display: "grid",
                "grid-template-columns": "repeat(2, minmax(0, 1fr))",
                gap: "6px",
                "align-content": "start",
              }}>
                <For each={selectedMetrics()}>{([k, v]) => (
                  <div style={{
                    padding: "7px 8px",
                    background: "rgba(217,119,87,0.055)",
                    border: "1px solid rgba(217,119,87,0.14)",
                    "border-radius": "6px",
                  }}>
                    <div style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)" }}>{k}</div>
                    <div style={{ "font-family": "var(--font-mono)", "font-size": "13px", color: "var(--ink-0)", "font-weight": 650 }}>{v}</div>
                  </div>
                )}</For>
              </div>
            </Show>
          </div>
        </Show>
      </CardShell>
      <Show when={open()}>
        <ArtifactModal
          title={info().title || tsCode}
          subtitle="Company profile"
          onClose={() => setOpen(false)}
        >
          <div style={{ display: "grid", "grid-template-columns": "280px minmax(0, 1fr)", gap: "28px" }}>
            <div style={{ display: "flex", "flex-direction": "column", gap: "10px" }}>
              <For each={info().basics}>{([k, v]) => (
                <div style={{
                  padding: "9px 10px",
                  background: "rgba(255,255,255,0.025)",
                  border: "1px solid var(--line-1)",
                  "border-radius": "6px",
                }}>
                  <div style={{ "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--ink-3)", "margin-bottom": "3px" }}>{k}</div>
                  <div style={{ "font-size": "13px", color: "var(--ink-1)", "line-height": 1.35 }}>{v}</div>
                </div>
              )}</For>
            </div>
            <div style={{ display: "flex", "flex-direction": "column", gap: "22px" }}>
              <div style={{
                "font-size": "14px",
                color: "var(--ink-2)",
                "line-height": 1.7,
                "max-width": "760px",
              }}>{info().intro}</div>
              <Show when={info().metrics.length > 0}>
                <div style={{ display: "grid", "grid-template-columns": "repeat(4, minmax(0, 1fr))", gap: "10px" }}>
                  <For each={info().metrics}>{([k, v]) => (
                    <div style={{
                      padding: "11px 12px",
                      background: "rgba(255,255,255,0.035)",
                      border: "1px solid rgba(255,255,255,0.10)",
                      "border-radius": "8px",
                    }}>
                      <div style={{ "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--ink-3)", "margin-bottom": "4px" }}>{k}</div>
                      <div style={{ "font-family": "var(--font-mono)", "font-size": "15px", color: "var(--ink-0)", "font-weight": 700 }}>{v}</div>
                    </div>
                  )}</For>
                </div>
              </Show>
            </div>
          </div>
        </ArtifactModal>
      </Show>
    </>
  );
}

/* ---------- get_financials ---------- */

type FinancialTab = "overview" | "statements" | "statistics" | "dividends" | "earnings" | "revenue";
type StatementTab = "income" | "balance" | "cashflow";
type PeriodMode = "quarterly" | "annual";

const FINANCIAL_TABS: Array<[FinancialTab, string]> = [
  ["overview", "概览"],
  ["statements", "三表"],
  ["statistics", "指标"],
  ["dividends", "分红"],
  ["earnings", "盈利"],
  ["revenue", "收入"],
];

const STATEMENT_TABS: Array<[StatementTab, string]> = [
  ["income", "利润表"],
  ["balance", "资产负债表"],
  ["cashflow", "现金流量表"],
];

function FinancialsCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const ticker = String(args.ticker ?? "");
  const market = String(args.market ?? "");
  const initialPeriods = typeof args.periods === "number" ? Math.max(1, Math.min(24, args.periods)) : 12;
  const [open, setOpen] = createSignal(false);
  const [periods, setPeriods] = createSignal(initialPeriods);
  const [periodMode, setPeriodMode] = createSignal<PeriodMode>("quarterly");
  const [tab, setTab] = createSignal<FinancialTab>("overview");
  const [statementTab, setStatementTab] = createSignal<StatementTab>("income");
  const [preview, setPreview] = createSignal(toolOutput(props.tool));
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal("");
  const tables = createMemo(() => parseMarkdownTables(preview()));
  const displayLabel = createMemo(() => securityLabel(preview()) || ticker);
  const incomeTable = () => findFinancialTable(tables(), ["利润表", "Income"]);
  const balanceTable = () => findFinancialTable(tables(), ["资产负债表", "Balance"]);
  const cashflowTable = () => findFinancialTable(tables(), ["现金流", "Cash Flow"]);
  const ratioTable = () => findFinancialTable(tables(), ["财务指标", "Ratios"]);
  const dividendTable = () => findFinancialTable(tables(), ["分红", "Dividend"]);
  const facts = createMemo(() => financialFacts(tables()));
  const activeStatementTable = () =>
    statementTab() === "income" ? incomeTable()
    : statementTab() === "balance" ? balanceTable()
    : cashflowTable();
  const needsFullFinancials = () => {
    if (tables().length === 0) return true;
    const requested = String(args.report_type ?? "all");
    if (requested === "all") return !incomeTable() || !balanceTable() || !cashflowTable() || !ratioTable();
    if (requested === "income") return !incomeTable();
    if (requested === "balance") return !balanceTable();
    if (requested === "cashflow") return !cashflowTable();
    if (requested === "ratios") return !ratioTable();
    return false;
  };

  const load = async (nextPeriods: number) => {
    if (!ticker || !market) return;
    setPeriods(nextPeriods);
    setLoading(true);
    setError("");
    try {
      const url = new URL("/api/v1/tools/financials", window.location.origin);
      url.searchParams.set("ticker", ticker);
      url.searchParams.set("market", market);
      url.searchParams.set("report_type", "all");
      url.searchParams.set("periods", String(nextPeriods));
      setPreview(await fetchToolPreview(url));
    } catch (e) {
      setError(parseMarkdownTables(preview()).length > 0 ? "" : e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  onMount(() => {
    if (props.tool.status === "completed" && ticker && market && needsFullFinancials()) {
      void load(periods());
    }
  });

  // The card's mini-charts are fine on the cached tool output, but the detail
  // modal should show the full current statements — the agent's cached call may
  // predate newer line items. Fetch once the first time the modal opens.
  let loadedFreshOnOpen = false;
  createEffect(() => {
    if (open() && !loadedFreshOnOpen && props.tool.status === "completed" && ticker && market) {
      loadedFreshOnOpen = true;
      void load(periods());
    }
  });

  return (
    <>
      <CardShell
        status={props.tool.status}
        badge={toolDisplayName(props.tool.name)}
        detail={displayLabel()}
        tool={props.tool}
        callId={props.tool.call_id}
        size="wide"
        onClick={() => setOpen(true)}
      >
        <div style={{ display: "flex", "align-items": "center", gap: "8px", "margin-bottom": "10px" }}>
          <PeriodButtons value={periods()} onPick={load} />
          <Show when={loading()}>
            <span style={{ "font-family": "var(--font-mono)", "font-size": "11.5px", color: "var(--ink-3)" }}>
              loading statements…
            </span>
          </Show>
          <Show when={error()}>
            <span style={{ "font-family": "var(--font-mono)", "font-size": "11.5px", color: "#d97070", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
              {error()}
            </span>
          </Show>
        </div>
        <Show
          when={tables().length > 0}
          fallback={
            <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
              <Show when={props.tool.status === "in_progress"} fallback="No financial tables.">
                Fetching financials…
              </Show>
            </div>
          }
        >
          <div style={{
            display: "grid",
            "grid-template-columns": "minmax(210px, 0.7fr) minmax(280px, 1.3fr)",
            gap: "12px",
          }}>
            <FinancialFactGrid facts={facts()} />
            <div style={{ display: "grid", "grid-template-columns": "repeat(auto-fit, minmax(135px, 1fr))", gap: "10px" }}>
              <FinancialMiniChart title="营业总收入" table={incomeTable()} columnHints={["营业总收入", "Total Revenue"]} color="var(--chart-2)" />
              <FinancialMiniChart title="ROE" table={ratioTable()} columnHints={["ROE"]} color="var(--chart-1)" />
              <FinancialMiniChart title="经营现金流" table={cashflowTable()} columnHints={["经营活动", "Operating CF"]} color="var(--chart-3)" />
              <FinancialMiniChart title="资产负债率" table={ratioTable()} columnHints={["资产负债率", "Debt"]} color="var(--chart-4)" />
            </div>
          </div>
          <div style={{
            "margin-top": "10px",
            "font-family": "var(--font-mono)",
            "font-size": "11px",
            color: "var(--clay-soft)",
          }}>
            点击查看完整财务面板
          </div>
        </Show>
      </CardShell>
      <Show when={open()}>
        <ArtifactModal
          title={displayLabel()}
          subtitle={`财务 · ${periods()} 期`}
          onClose={() => setOpen(false)}
        >
          <div class="lk-fin-modal">
            <div class="lk-fin-toolbar">
              <div class="lk-fin-tabs">
                <For each={FINANCIAL_TABS}>{([id, label]) => (
                  <button type="button" class={tab() === id ? "active" : ""} onClick={() => setTab(id)}>
                    {label}
                  </button>
                )}</For>
              </div>
              <div class="lk-fin-controls">
                <PeriodModeToggle value={periodMode()} onPick={setPeriodMode} />
                <PeriodButtons value={periods()} onPick={load} />
                <Show when={loading()}>
	                  <span class="lk-fin-note">加载中…</span>
                </Show>
                <Show when={error()}>
                  <span class="lk-fin-note error">{error()}</span>
                </Show>
              </div>
            </div>

            <Show when={tab() === "overview"}>
              <FinancialOverview
                facts={facts()}
                income={incomeTable()}
                balance={balanceTable()}
                cashflow={cashflowTable()}
                ratios={ratioTable()}
                mode={periodMode()}
                periods={periods()}
              />
            </Show>

            <Show when={tab() === "statements"}>
              <div class="lk-fin-subtabs">
                <For each={STATEMENT_TABS}>{([id, label]) => (
                  <button type="button" class={statementTab() === id ? "active" : ""} onClick={() => setStatementTab(id)}>
                    {label}
                  </button>
                )}</For>
              </div>
              <FinancialStatementView
                table={activeStatementTable()}
                statement={statementTab()}
                mode={periodMode()}
                periods={periods()}
              />
            </Show>

            <Show when={tab() === "statistics"}>
              <FinancialStatisticsView table={ratioTable()} mode={periodMode()} periods={periods()} />
            </Show>

            <Show when={tab() === "dividends"}>
              <FinancialDividendsView table={dividendTable()} mode={periodMode()} periods={periods()} />
            </Show>

            <Show when={tab() === "earnings"}>
              <FinancialEarningsView income={incomeTable()} ratios={ratioTable()} mode={periodMode()} periods={periods()} />
            </Show>

            <Show when={tab() === "revenue"}>
              <FinancialRevenueView table={incomeTable()} mode={periodMode()} periods={periods()} />
            </Show>
          </div>
        </ArtifactModal>
      </Show>
    </>
  );
}

function PeriodButtons(props: { value: number; onPick: (periods: number) => void }) {
  const options = createMemo(() => {
    const base = [8, 12, 24];
    if (base.includes(props.value)) return base;
    return [props.value, ...base.filter((n) => n !== props.value)].slice(0, 4);
  });
  return (
    <div style={{
      display: "inline-flex",
      gap: "4px",
      padding: "2px",
      background: "rgba(255,255,255,0.03)",
      border: "1px solid var(--line-1)",
      "border-radius": "6px",
    }}>
      <For each={options()}>{(n) => (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); if (props.value !== n) props.onPick(n); }}
          style={{
            appearance: "none",
            border: "0",
            background: props.value === n ? "rgba(217,119,87,0.18)" : "transparent",
            color: props.value === n ? "var(--clay-soft)" : "var(--ink-3)",
            "font-family": "var(--font-mono)",
            "font-size": "11.5px",
            padding: "4px 8px",
            "border-radius": "4px",
            cursor: "pointer",
          }}
        >{n}</button>
      )}</For>
    </div>
  );
}

function PeriodModeToggle(props: { value: PeriodMode; onPick: (mode: PeriodMode) => void }) {
  return (
    <div class="lk-fin-period-toggle">
      <button type="button" class={props.value === "annual" ? "active" : ""} onClick={() => props.onPick("annual")}>年报</button>
      <button type="button" class={props.value === "quarterly" ? "active" : ""} onClick={() => props.onPick("quarterly")}>季报</button>
    </div>
  );
}

function FinancialOverview(props: {
  facts: Array<[string, string]>;
  income?: MarkdownTable;
  balance?: MarkdownTable;
  cashflow?: MarkdownTable;
  ratios?: MarkdownTable;
  mode: PeriodMode;
  periods: number;
}) {
  return (
    <div class="lk-fin-tab-body">
      <section class="lk-fin-keyfacts">
        <For each={props.facts.slice(0, 10)}>{([label, value]) => (
          <div>
            <span>{label}</span>
            <b>{value}</b>
          </div>
        )}</For>
      </section>
      <div class="lk-fin-chart-grid">
        <FinancialSeriesChart
          title="增长与盈利"
          table={props.income}
          mode={props.mode}
          periods={props.periods}
          series={[
            { label: "营业总收入", hints: ["营业总收入", "Total Revenue"], color: "var(--chart-2)" },
            { label: "归母净利", hints: ["归母净利", "Net Income"], color: "var(--chart-3)" },
          ]}
        />
        <FinancialSeriesChart
          title="收入到利润"
          table={props.income}
          mode={props.mode}
          periods={props.periods}
          series={[
            { label: "营业总收入", hints: ["营业总收入", "Total Revenue"], color: "var(--chart-3)" },
            { label: "营业利润", hints: ["营业利润", "Operating Income"], color: "var(--chart-2)" },
            { label: "归母净利", hints: ["归母净利", "Net Income"], color: "var(--chart-4)" },
          ]}
        />
        <FinancialSeriesChart
          title="资产负债"
          table={props.balance}
          mode={props.mode}
          periods={props.periods}
          series={[
            { label: "总资产", hints: ["总资产", "Total Assets"], color: "var(--chart-2)" },
            { label: "总负债", hints: ["总负债", "Total Liabilities"], color: "var(--chart-3)" },
            { label: "总债务", hints: ["总债务", "Total Debt"], color: "var(--chart-4)" },
          ]}
        />
        <FinancialSeriesChart
          title="质量指标"
          table={props.ratios}
          mode={props.mode}
          periods={props.periods}
          series={[
            { label: "ROE", hints: ["ROE"], color: "var(--chart-1)" },
            { label: "毛利率", hints: ["毛利率", "Gross Margin"], color: "var(--chart-3)" },
            { label: "资产负债率", hints: ["资产负债率", "Debt"], color: "var(--chart-4)" },
          ]}
        />
      </div>
    </div>
  );
}

function FinancialStatementView(props: {
  table?: MarkdownTable;
  statement: StatementTab;
  mode: PeriodMode;
  periods: number;
}) {
  const defaults = () =>
    props.statement === "balance" ? ["总资产", "总负债"]
    : props.statement === "cashflow" ? ["经营活动", "投资活动", "筹资活动"]
    : ["营业总收入", "营业利润", "归母净利"];
  return (
    <FinancialLinkedView
      title="报表趋势"
      table={props.table}
      mode={props.mode}
      periods={props.periods}
      defaults={defaults()}
      height={210}
    />
  );
}

function FinancialStatisticsView(props: { table?: MarkdownTable; mode: PeriodMode; periods: number }) {
  return (
    <FinancialLinkedView
      title="盈利能力"
      table={props.table}
      mode={props.mode}
      periods={props.periods}
      defaults={["ROE", "毛利率", "净利率"]}
      grouped
      height={210}
    />
  );
}

function FinancialDividendsView(props: { table?: MarkdownTable; mode: PeriodMode; periods: number }) {
  return (
    <FinancialLinkedView
      title="分红历史"
      table={props.table}
      mode={props.mode}
      periods={props.periods}
      defaults={["每股现金分红税前", "每股现金分红税后"]}
      ignoreMode
      height={210}
    />
  );
}

function FinancialEarningsView(props: {
  income?: MarkdownTable;
  ratios?: MarkdownTable;
  mode: PeriodMode;
  periods: number;
}) {
  return (
    <FinancialLinkedView
      title="盈利质量"
      table={props.ratios}
      mode={props.mode}
      periods={props.periods}
      defaults={["EPS", "ROE", "净利率"]}
      rowHints={["EPS", "BPS", "ROE", "ROA", "净利率", "毛利率", "每股", "Per Share", "Margin"]}
      height={230}
    />
  );
}

function FinancialRevenueView(props: { table?: MarkdownTable; mode: PeriodMode; periods: number }) {
  return (
    <FinancialLinkedView
      title="收入与利润"
      table={props.table}
      mode={props.mode}
      periods={props.periods}
      defaults={["营业总收入", "营业收入", "毛利", "归母净利"]}
      rowHints={["营业总收入", "营业收入", "Revenue", "毛利", "Gross"]}
      height={230}
    />
  );
}

function findFinancialTable(tables: MarkdownTable[], needles: string[]): MarkdownTable | undefined {
  return tables.find((table) =>
    needles.some((needle) => table.title.toLowerCase().includes(needle.toLowerCase()))
  );
}

function headerIndex(headers: string[], hints: string[]): number {
  return headers.findIndex((h) =>
    hints.some((hint) => h.toLowerCase().includes(hint.toLowerCase()))
  );
}

function financialCell(table: MarkdownTable | undefined, hints: string[]): string {
  if (!table || table.rows.length === 0) return "";
  const idx = headerIndex(table.headers, hints);
  if (idx < 0) return "";
  return table.rows[0]?.[idx] ?? "";
}

function financialFacts(tables: MarkdownTable[]): Array<[string, string]> {
  const income = findFinancialTable(tables, ["利润表", "Income"]);
  const ratio = findFinancialTable(tables, ["财务指标", "Ratios"]);
  const balance = findFinancialTable(tables, ["资产负债表", "Balance"]);
  const cashflow = findFinancialTable(tables, ["现金流", "Cash Flow"]);
  const facts: Array<[string, string]> = [];
  const add = (label: string, value: string) => {
    if (value && value !== "—" && facts.length < 12) facts.push([label, value]);
  };
  add("报告期", income?.rows[0]?.[0] || ratio?.rows[0]?.[0] || "");
  add("营业总收入", financialCell(income, ["营业总收入", "Total Revenue"]));
  add("营业收入", financialCell(income, ["营业收入", "Revenue"]));
  add("净利", financialCell(income, ["归母净利", "Net Income"]));
  add("EPS", financialCell(ratio, ["EPS"]));
  add("ROE", financialCell(ratio, ["ROE"]));
  add("毛利率", financialCell(ratio, ["毛利率", "Gross Margin"]));
  add("负债率", financialCell(ratio, ["资产负债率", "Debt"]));
  add("流动比率", financialCell(ratio, ["流动比率", "Current Ratio"]));
  add("总资产", financialCell(balance, ["总资产", "Total Assets"]));
  add("经营CF", financialCell(cashflow, ["经营活动", "Operating CF"]));
  return facts;
}

function FinancialFactGrid(props: { facts: Array<[string, string]> }) {
  return (
    <div style={{ display: "grid", "grid-template-columns": "repeat(2, minmax(0, 1fr))", gap: "8px", "align-content": "start" }}>
      <For each={props.facts.slice(0, 8)}>{([label, value]) => (
        <MetricTile label={label} value={value} />
      )}</For>
    </div>
  );
}

function MetricTile(props: { label: string; value: string }) {
  return (
    <div style={{
      padding: "9px 10px",
      background: "rgba(255,255,255,0.03)",
      border: "1px solid rgba(255,255,255,0.09)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{ "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--ink-3)", "margin-bottom": "4px" }}>
        {props.label}
      </div>
      <div style={{
        "font-family": "var(--font-mono)",
        "font-size": "13px",
        color: "var(--ink-0)",
        "font-weight": 700,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>
        {props.value}
      </div>
    </div>
  );
}

function financialNumber(raw: string): number {
  const trimmed = raw.replace(/[,，%]/g, "").trim();
  const match = trimmed.match(/[-+]?\d+(?:\.\d+)?/);
  if (!match) return NaN;
  const n = parseFloat(match[0]);
  if (!Number.isFinite(n)) return NaN;
  if (/B$/i.test(trimmed)) return n * 1_000;
  if (/M$/i.test(trimmed)) return n;
  return n;
}

function isAnnualPeriod(label: string): boolean {
  const s = label.trim();
  return /^\d{4}$/.test(s) || /^\d{4}[-/]?12$/.test(s) || /\bFY\b/i.test(s);
}

// A-share Q4 closes on 12-31, so its report period (yyyy-12) is simultaneously the
// fourth quarter and the annual figure. Annual mode wants only the year-end snapshot,
// but quarterly mode must keep every quarter end — including 12 — rather than excluding
// annuals, otherwise Q4 silently vanishes from the quarterly series.
function isQuarterlyPeriod(label: string): boolean {
  return /^\d{4}[-/](0?3|0?6|0?9|12)\b/.test(label.trim());
}

function tableForMode(table: MarkdownTable | undefined, mode: PeriodMode): MarkdownTable | undefined {
  if (!table) return undefined;
  const filtered = table.rows.filter((row) =>
    mode === "annual" ? isAnnualPeriod(row[0] ?? "") : isQuarterlyPeriod(row[0] ?? "")
  );
  return { ...table, rows: filtered.length > 0 ? filtered : table.rows };
}

function columnIndex(table: MarkdownTable | undefined, hints: string[]): number {
  if (!table) return -1;
  return headerIndex(table.headers, hints);
}

interface FinancialSeriesSpec {
  label: string;
  hints: string[];
  color: string;
}

function FinancialSeriesChart(props: {
  title: string;
  table?: MarkdownTable;
  mode: PeriodMode;
  series: FinancialSeriesSpec[];
  height?: number;
  periods?: number;
  ignoreMode?: boolean;
}) {
  const plot = () => {
    const table = props.ignoreMode ? props.table : tableForMode(props.table, props.mode);
    if (!table) return { rows: [], series: [] as Array<FinancialSeriesSpec & { idx: number }> };
    const series = props.series
      .map((s) => ({ ...s, idx: columnIndex(table, s.hints) }))
      .filter((s) => s.idx >= 0);
    // Match the period count chosen in the toolbar so the chart and the table
    // below it always show the same number of periods.
    const rows = table.rows
      .slice(0, props.periods ?? 12)
      .map((row) => ({
        label: row[0] ?? "",
        values: series.map((s) => financialNumber(row[s.idx] ?? "")),
        raw: series.map((s) => (row[s.idx] ?? "—").trim() || "—"),
      }))
      .filter((row) => row.values.some((v) => Number.isFinite(v)))
      .reverse();
    return { rows, series };
  };
  const height = () => props.height ?? 180;
  const chart = () => {
    const rows = plot().rows;
    const max = Math.max(1, ...rows.flatMap((row) => row.values.map((v) => Math.abs(v)).filter(Number.isFinite)));
    const w = 760;
    const h = height();
    const pad = { l: 36, r: 18, t: 16, b: 34 };
    const plotW = w - pad.l - pad.r;
    const plotH = h - pad.t - pad.b;
    return { rows, max, w, h, pad, plotW, plotH };
  };
  let cardEl: HTMLElement | undefined;
  const [tip, setTip] = createSignal<
    { x: number; y: number; period: string; label: string; value: string; color: string } | null
  >(null);
  return (
    <section class="lk-fin-chart-card" ref={(el) => (cardEl = el)}>
      <div class="lk-fin-section-head">
        <span>{props.title}</span>
	        <em>{props.mode === "annual" ? "年报" : "季报"}</em>
      </div>
      <Show
        when={plot().rows.length > 0 && plot().series.length > 0}
	        fallback={<FinancialEmpty label="暂无可绘制序列" />}
      >
        <svg class="lk-fin-svg" viewBox={`0 0 ${chart().w} ${chart().h}`} preserveAspectRatio="none">
          <For each={[0, 1, 2, 3]}>{(n) => {
            const y = chart().pad.t + (chart().plotH / 3) * n;
            return <line x1={chart().pad.l} x2={chart().w - chart().pad.r} y1={y} y2={y} class="grid" />;
          }}</For>
          <For each={plot().rows}>{(row, rowIndex) => {
            const c = chart();
            const groupW = c.plotW / Math.max(1, c.rows.length);
            const series = plot().series;
            const barW = Math.max(8, Math.min(34, groupW / Math.max(1, series.length + 1)));
            const x0 = c.pad.l + rowIndex() * groupW + (groupW - barW * series.length) / 2;
            return (
              <>
                <For each={series}>{(s, sIndex) => {
                  const value = row.values[sIndex()];
                  const barH = Number.isFinite(value) ? Math.max(2, Math.abs(value) / c.max * c.plotH) : 0;
                  const y = c.pad.t + c.plotH - barH;
                  return (
                    <rect
                      class="lk-fin-bar"
                      x={x0 + sIndex() * barW}
                      y={y}
                      width={barW * 0.74}
                      height={barH}
                      rx="2"
                      style={{ fill: s.color }}
                      opacity="0.88"
                      onMouseMove={(e) => {
                        if (!cardEl) return;
                        const r = cardEl.getBoundingClientRect();
                        setTip({
                          x: e.clientX - r.left,
                          y: e.clientY - r.top,
                          period: row.label,
                          label: s.label,
                          value: row.raw[sIndex()],
                          color: s.color,
                        });
                      }}
                      onMouseLeave={() => setTip(null)}
                    />
                  );
                }}</For>
                <text x={c.pad.l + rowIndex() * groupW + groupW / 2} y={c.h - 10} text-anchor="middle" class="axis">
                  {shortPeriodLabel(row.label)}
                </text>
              </>
            );
          }}</For>
        </svg>
        <div class="lk-fin-legend">
          <For each={plot().series}>{(s) => (
            <span><i style={{ background: s.color }} />{s.label}</span>
          )}</For>
        </div>
        <Show when={tip()}>
          {(t) => (
            <div class="lk-fin-tip" style={{ left: `${t().x}px`, top: `${t().y}px` }}>
              <span class="tip-period">{t().period}</span>
              <span class="tip-metric"><i style={{ background: t().color }} />{t().label}</span>
              <b>{t().value}</b>
            </div>
          )}
        </Show>
      </Show>
    </section>
  );
}

function shortPeriodLabel(label: string): string {
  const q = label.match(/^(\d{4})[-/](0?3|0?6|0?9|12)$/);
  if (q) {
    const quarter = q[2] === "03" || q[2] === "3" ? "Q1" : q[2] === "06" || q[2] === "6" ? "Q2" : q[2] === "09" || q[2] === "9" ? "Q3" : "Q4";
    return `${quarter} '${q[1].slice(2)}`;
  }
  return label;
}

function FinancialMatrixTable(props: {
  table?: MarkdownTable;
  mode: PeriodMode;
  periods: number;
  rowHints?: string[];
  grouped?: boolean;
  /** Interactive mode: when set, rows toggle chart series. `colorOf` returns
   *  the row's plotted color (or null when not plotted); `onToggle` flips it. */
  colorOf?: (header: string) => string | null;
  onToggle?: (header: string) => void;
  ignoreMode?: boolean;
}) {
  const fallbackColors = ["var(--chart-2)", "var(--chart-3)", "var(--chart-1)", "var(--chart-4)"];
  const view = () => {
    const table = props.ignoreMode ? props.table : tableForMode(props.table, props.mode);
    if (!table) return { periods: [] as string[], metrics: [] as Array<{ label: string; header: string; values: string[]; color: string | null }> };
    const rows = table.rows.slice(0, props.periods).reverse();
    const metricHeaders = table.headers.slice(1);
    const filtered = props.rowHints?.length
      ? metricHeaders.filter((h) => props.rowHints!.some((hint) => h.toLowerCase().includes(hint.toLowerCase())))
      : metricHeaders;
    return {
      periods: rows.map((row) => row[0] ?? ""),
      metrics: filtered.slice(0, props.grouped ? 48 : 40).map((header, i) => {
        const idx = table.headers.indexOf(header);
        return {
          label: header,
          header,
          values: rows.map((row) => row[idx] ?? "—"),
          color: props.colorOf ? props.colorOf(header) : fallbackColors[i % fallbackColors.length],
        };
      }),
    };
  };
  const interactive = () => !!props.onToggle;
  return (
    <section class="lk-fin-table-card">
	      <Show when={view().metrics.length > 0} fallback={<FinancialEmpty label="暂无表格数据" />}>
        <Show when={interactive()}>
          <div class="lk-fin-matrix-hint">点击指标行可切换是否绘制到上方图表（最多 4 项）</div>
        </Show>
        <div class="lk-fin-matrix">
          <table>
            <thead>
              <tr>
	                <th>币种：CNY</th>
                <For each={view().periods}>{(period) => <th>{period}</th>}</For>
              </tr>
            </thead>
            <tbody>
              <For each={view().metrics}>{(metric) => (
                <tr
                  class={interactive() ? "is-toggleable" : ""}
                  data-on={interactive() ? (metric.color != null ? "true" : "false") : undefined}
                  onClick={props.onToggle ? () => props.onToggle!(metric.header) : undefined}
                  title={interactive() ? (metric.color != null ? "点击从图表移除" : "点击绘制到图表") : undefined}
                >
                  <td>
                    <i
                      class="lk-fin-mark"
                      style={metric.color ? { background: metric.color, "border-color": metric.color } : { background: "transparent" }}
                    />
                    {metric.label}
                  </td>
                  <For each={metric.values}>{(value) => <td>{value}</td>}</For>
                </tr>
              )}</For>
            </tbody>
          </table>
        </div>
      </Show>
    </section>
  );
}

function stripUnit(header: string): string {
  const s = header.replace(/（[^）]*）/g, "").replace(/\([^)]*\)/g, "").replace(/%$/, "").trim();
  return s || header;
}

// Fixed per-row palette: a metric's colour is decided by its position in the
// table, NOT by selection order, so toggling one row never recolours another.
const LINKED_PALETTE = ["var(--chart-2)", "var(--chart-3)", "var(--chart-1)", "var(--chart-4)", "#c98ab8", "#6fb0b5"];
const LINKED_MAX_SERIES = 4;

/** Chart + table where the table rows drive which series the chart plots.
 *  Starts on `defaults`, caps at 4 series, and keeps each row's marker color in
 *  sync with its bar color. */
function FinancialLinkedView(props: {
  title: string;
  table?: MarkdownTable;
  mode: PeriodMode;
  periods: number;
  defaults: string[];
  rowHints?: string[];
  grouped?: boolean;
  height?: number;
  ignoreMode?: boolean;
}) {
  const metricHeaders = createMemo(() => {
    const t = props.ignoreMode ? props.table : tableForMode(props.table, props.mode);
    if (!t) return [] as string[];
    const hs = t.headers.slice(1);
    return props.rowHints?.length
      ? hs.filter((h) => props.rowHints!.some((hint) => h.toLowerCase().includes(hint.toLowerCase())))
      : hs;
  });
  const defaultSelection = createMemo(() => {
    const hs = metricHeaders();
    const sel: string[] = [];
    for (const d of props.defaults) {
      const match = hs.find((h) => h.toLowerCase().includes(d.toLowerCase()));
      if (match && !sel.includes(match)) sel.push(match);
      if (sel.length >= LINKED_MAX_SERIES) break;
    }
    if (sel.length === 0 && hs.length > 0) sel.push(hs[0]);
    return sel;
  });
  const [touched, setTouched] = createSignal(false);
  const [selected, setSelected] = createSignal<string[]>([]);
  // Track defaults until the user picks. When the underlying metric set changes
  // (e.g. switching statement sub-tabs), the default selection changes too —
  // reset to it and forget the user's prior picks, which no longer apply.
  let prevDefaultsKey = "";
  createEffect(() => {
    const defaults = defaultSelection();
    const key = defaults.join("|");
    if (key !== prevDefaultsKey) {
      prevDefaultsKey = key;
      setTouched(false);
      setSelected(defaults);
    } else if (!touched()) {
      setSelected(defaults);
    }
  });
  // Colour is fixed by the metric's row index (stable across toggles) and only
  // shown once the row is selected/plotted.
  const colorOf = (header: string): string | null => {
    if (!selected().includes(header)) return null;
    const i = metricHeaders().indexOf(header);
    return LINKED_PALETTE[(i >= 0 ? i : 0) % LINKED_PALETTE.length];
  };
  function toggle(header: string) {
    setTouched(true);
    setSelected((prev) => {
      if (prev.includes(header)) return prev.filter((h) => h !== header);
      if (prev.length >= LINKED_MAX_SERIES) return prev;
      return [...prev, header];
    });
  }
  const series = () =>
    selected().map((h) => ({ label: stripUnit(h), hints: [h], color: colorOf(h) ?? "var(--chart-2)" }));
  return (
    <div class="lk-fin-tab-body">
      <FinancialSeriesChart
        title={props.title}
        table={props.table}
        mode={props.mode}
        periods={props.periods}
        series={series()}
        height={props.height}
        ignoreMode={props.ignoreMode}
      />
      <FinancialMatrixTable
        table={props.table}
        mode={props.mode}
        periods={props.periods}
        rowHints={props.rowHints}
        grouped={props.grouped}
        colorOf={colorOf}
        onToggle={toggle}
        ignoreMode={props.ignoreMode}
      />
    </div>
  );
}

function FinancialEmpty(props: { label: string }) {
  return <div class="lk-fin-empty"><EmptyHint text={props.label} /></div>;
}

/** Shared inline empty state — a quiet monoline mark + one explanation line.
 *  Sized for an in-card section, not a page (state-coverage.md). */
function EmptyHint(props: { text: string; sub?: string }) {
  return (
    <div class="lk-empty-hint">
      <svg class="ic" viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
        <path d="M5 5.5h14M5 11h9M5 16.5h5" />
      </svg>
      <div class="lk-empty-hint-text">{props.text}</div>
      <Show when={props.sub}>
        <div class="lk-empty-hint-sub">{props.sub}</div>
      </Show>
    </div>
  );
}

/** A table the backend returned with no usable rows — either truly empty, or a
 *  single placeholder row whose cells are all dashes / "无返回记录" / "unavailable"
 *  markers. Routes those to the empty state instead of a hollow table. */
function tableIsEffectivelyEmpty(table?: MarkdownTable): boolean {
  if (!table || table.rows.length === 0) return true;
  const blank = (c: string) => {
    const v = (c ?? "").trim();
    return v === "" || /^[-—–·.]+$/.test(v) ||
      /无返回记录|无数据|暂无|不可用|unavailable|no data|n\/a/i.test(v);
  };
  return table.rows.every((row) => row.every(blank));
}

function FinancialMiniChart(props: {
  title: string;
  table?: MarkdownTable;
  columnHints: string[];
  color: string;
  large?: boolean;
}) {
  const rows = () => {
    if (!props.table) return [];
    const idx = headerIndex(props.table.headers, props.columnHints);
    if (idx < 0) return [];
    return props.table.rows
      .slice(0, props.large ? 12 : 8)
      .map((row) => ({ label: row[0] ?? "", value: financialNumber(row[idx] ?? "") }))
      .filter((row) => Number.isFinite(row.value))
      .reverse();
  };
  const maxAbs = () => Math.max(1, ...rows().map((row) => Math.abs(row.value)));
  return (
    <div style={{
      padding: props.large ? "13px 14px" : "9px 10px",
      background: "rgba(255,255,255,0.02)",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{ display: "flex", "align-items": "baseline", gap: "8px", "margin-bottom": "10px" }}>
        <div style={{ "font-size": props.large ? "13px" : "11.5px", color: "var(--ink-0)", "font-weight": 650 }}>
          {props.title}
        </div>
        <Show when={rows().length > 0}>
          <div style={{ "margin-left": "auto", "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--ink-3)" }}>
            {rows()[rows().length - 1]?.label}
          </div>
        </Show>
      </div>
      <Show
        when={rows().length > 0}
	        fallback={<div style={{ height: props.large ? "120px" : "68px", display: "grid", "place-items": "center" }}><EmptyHint text="暂无序列" /></div>}
      >
        <div style={{
          height: props.large ? "130px" : "76px",
          display: "grid",
          "grid-template-columns": `repeat(${Math.max(1, rows().length)}, minmax(0, 1fr))`,
          gap: props.large ? "8px" : "5px",
          "align-items": "end",
          "border-bottom": "1px solid rgba(255,255,255,0.08)",
          padding: "0 2px 1px",
        }}>
          <For each={rows()}>{(row) => {
            const height = Math.max(4, Math.round(Math.abs(row.value) / maxAbs() * (props.large ? 112 : 62)));
            const color = row.value >= 0 ? props.color : "var(--down)";
            return (
              <div title={`${row.label}: ${row.value}`} style={{ display: "flex", "align-items": "end", height: "100%" }}>
                <div style={{
                  width: "100%",
                  height: `${height}px`,
                  background: color,
                  opacity: 0.78,
                  "border-radius": "3px 3px 0 0",
                }} />
              </div>
            );
          }}</For>
        </div>
      </Show>
    </div>
  );
}

function FinancialTablePane(props: {
  table: MarkdownTable;
  rowsLimit?: number;
  colsLimit?: number;
}) {
  const cols = () => props.table.headers.slice(0, props.colsLimit ?? 4);
  const rows = () => props.table.rows.slice(0, props.rowsLimit ?? 4);
  return (
    <div style={{
      padding: "9px 10px",
      background: "rgba(255,255,255,0.02)",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
      overflow: "auto",
    }}>
      <div style={{
        "font-size": "13px",
        color: "var(--ink-0)",
        "font-weight": 650,
        "margin-bottom": "7px",
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>
        {props.table.title.replace(/^.+?·\s*/, "")}
      </div>
      <table style={{
        width: "100%",
        "border-collapse": "collapse",
        "font-family": "var(--font-mono)",
        "font-size": "11px",
        "min-width": cols().length > 5 ? "720px" : "0",
      }}>
        <thead>
          <tr>
            <For each={cols()}>{(h) => (
              <th style={{
                color: "var(--ink-3)",
                "font-weight": 600,
                "text-align": h === cols()[0] ? "left" : "right",
                padding: "0 0 5px",
              }}>{h}</th>
            )}</For>
          </tr>
        </thead>
        <tbody>
          <For each={rows()}>{(row) => (
            <tr>
              <For each={cols()}>{(_, i) => (
                <td style={{
                  color: i() === 0 ? "var(--ink-2)" : "var(--ink-0)",
                  "text-align": i() === 0 ? "left" : "right",
                  padding: "4px 0",
                  "border-top": "1px solid rgba(255,255,255,0.045)",
                  "white-space": "nowrap",
                }}>
                  {row[i()] ?? "—"}
                </td>
              )}</For>
            </tr>
          )}</For>
        </tbody>
      </table>
    </div>
  );
}

/* ---------- generic markdown context tools ---------- */

interface MarkdownSection {
  title: string;
  rows: string[];
  unavailable: boolean;
}

function MarkdownContextCards(props: { tool: ToolCallView; badge: string }) {
  const preview = () => toolOutput(props.tool);
  const title = () => firstHeading(preview());
  const tables = createMemo(() => parseMarkdownTables(preview()));
  const sections = createMemo(() => parseMarkdownSections(preview()));
  const overviewRows = createMemo(() => markdownTopRows(preview()));
  const sectionTitles = () => new Set(sections().map((section) => section.title));
  const standaloneTables = () => tables().filter((table) => !sectionTitles().has(table.title));
  const hasStructured = () =>
    Boolean(title()) || overviewRows().length > 0 || sections().length > 0 || standaloneTables().length > 0;

  return (
    <Show when={hasStructured()} fallback={<GenericToolCard tool={props.tool} />}>
      <>
        <Show when={title() || overviewRows().length > 0}>
          <CardShell
            status={props.tool.status}
            badge={props.badge}
            detail={toolPrimaryDetail(props.tool) || title()}
            tool={props.tool}
            callId={props.tool.call_id}
            size="wide"
          >
            <Show
              when={overviewRows().length > 0}
              fallback={<div style={{ color: "var(--ink-2)", "font-size": "13px" }}>{title()}</div>}
            >
              <MarkdownBulletList rows={overviewRows()} limit={6} />
            </Show>
          </CardShell>
        </Show>
        <For each={sections().slice(0, 10)}>{(section, index) => {
          const table = () => tables().find((candidate) => candidate.title === section.title);
          const [sectionTitle, sectionDetail] = splitSectionTitle(section.title);
          return (
            <CardShell
              status={props.tool.status}
              badge={sectionTitle}
              detail={section.unavailable ? "来源缺口" : sectionDetail}
              tool={props.tool}
              callId={`${props.tool.call_id}:section:${index()}`}
              size={table() ? "wide" : "normal"}
            >
              <Show
                when={table()}
                fallback={<MarkdownBulletList rows={section.rows} limit={8} />}
              >
                <FinancialTablePane table={table()!} rowsLimit={8} colsLimit={8} />
                <Show when={section.rows.length > 0}>
                  <div style={{ "margin-top": "8px" }}>
                    <MarkdownBulletList rows={section.rows} limit={4} />
                  </div>
                </Show>
              </Show>
            </CardShell>
          );
        }}</For>
        <For each={standaloneTables().slice(0, 6)}>{(table, index) => (
          <MarkdownTableCard
            tool={props.tool}
            badge={table.title.replace(/^.+?·\s*/, "") || props.badge}
            detail={toolPrimaryDetail(props.tool)}
            table={table}
            callId={`${props.tool.call_id}:table:${index()}`}
            size="wide"
            rowsLimit={8}
            colsLimit={8}
            emptyText="暂无结构化数据。"
          />
        )}</For>
      </>
    </Show>
  );
}

function MarkdownBulletList(props: { rows: string[]; limit: number }) {
  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
      <For each={props.rows.slice(0, props.limit)}>{(row) => (
        <div style={{
          color: row.includes("Source unavailable") || row.includes("unavailable") || row.includes("来源不可用") ? "#d97070" : "var(--ink-2)",
          "font-size": "12.5px",
          "line-height": 1.45,
          overflow: "hidden",
          "text-overflow": "ellipsis",
          display: "-webkit-box",
          "-webkit-line-clamp": "3",
          "-webkit-box-orient": "vertical",
        }}>
          {row}
        </div>
      )}</For>
    </div>
  );
}

function parseMarkdownSections(md: string): MarkdownSection[] {
  const sections: MarkdownSection[] = [];
  let current: MarkdownSection | null = null;
  let inTable = false;
  for (const rawLine of md.split("\n")) {
    const line = rawLine.trim();
    const h3 = line.match(/^###\s+(.+)$/);
    if (h3) {
      current = { title: h3[1].trim(), rows: [], unavailable: h3[1].includes("unavailable") || h3[1].includes("不可用") };
      sections.push(current);
      inTable = false;
      continue;
    }
    if (line.match(/^##\s+(.+)$/)) {
      current = null;
      inTable = false;
      continue;
    }
    if (!current || !line) continue;
    if (line.startsWith("|")) {
      inTable = true;
      continue;
    }
    if (inTable && line.startsWith("_")) {
      inTable = false;
    }
    if (inTable) continue;
    if (line.startsWith("_来源:") || line.startsWith("_Source:") || line.startsWith("_Caveat:")) continue;
    const text = line.replace(/^-\s+/, "");
    if (!text) continue;
    current.rows.push(text);
    if (text.includes("Source unavailable") || text.includes("unavailable") || text.includes("来源不可用")) current.unavailable = true;
  }
  return sections;
}

function markdownTopRows(md: string): string[] {
  const rows: string[] = [];
  let afterTitle = false;
  for (const rawLine of md.split("\n")) {
    const line = rawLine.trim();
    if (line.startsWith("### ")) break;
    if (line.startsWith("## ")) {
      afterTitle = true;
      continue;
    }
    if (!afterTitle || !line || line.startsWith("|") || line.includes("---")) continue;
    if (line.startsWith("_来源:") || line.startsWith("_Source:") || line.startsWith("_Caveat:")) continue;
    rows.push(line.replace(/^-\s+/, ""));
  }
  return rows;
}

function splitSectionTitle(title: string): [string, string] {
  const parts = title.split("·").map((part) => part.trim()).filter(Boolean);
  if (parts.length <= 1) return [title, ""];
  return [parts[0], parts.slice(1).join(" · ").replace(/`/g, "")];
}

function toolPrimaryDetail(tool: ToolCallView): string {
  const args = parseArgs(tool);
  for (const key of ["ts_code", "ticker", "symbol", "fund_code", "index_code", "coin_id", "mode"]) {
    if (typeof args[key] === "string" && String(args[key]).trim()) return String(args[key]);
  }
  if (Array.isArray(args.ts_codes)) return (args.ts_codes as string[]).slice(0, 3).join(", ");
  return "";
}

/* ---------- get_a_share_industry_context ---------- */

function AShareIndustryContextCards(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const subject = String(args.ts_code ?? args.industry_code ?? "");
  const preview = () => toolOutput(props.tool);
  const tables = createMemo(() => parseMarkdownTables(preview()));
  const summaryItems = createMemo(() => markdownSectionBullets(preview(), "任务摘要"));
  const classificationTable = () => findMarkdownTable(tables(), ["申万分类"]);
  const indexTable = () => findMarkdownTable(tables(), ["行业指数"]);
  const peersTable = () => findMarkdownTable(tables(), ["同业样本"]);
  const flowTable = () => findMarkdownTable(tables(), ["行业资金面"]);
  const hasStructured = () => summaryItems().length > 0 || tables().length > 0;
  const flowUnavailable = () =>
    flowTable()?.rows.some((row) => row.join(" ").includes("Source unavailable") || row.join(" ").includes("来源不可用") || row.join(" ").includes("暂不可用"));

  return (
    <Show when={hasStructured()} fallback={<GenericToolCard {...props} />}>
      <>
        <CardShell
          status={props.tool.status}
          badge={toolDisplayName(props.tool.name)}
          detail={subject}
          tool={props.tool}
          callId={props.tool.call_id}
          size="wide"
        >
          <Show
            when={summaryItems().length > 0}
            fallback={
              <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
                <Show when={props.tool.status === "in_progress"} fallback="暂无行业上下文摘要。">
                  正在获取行业上下文…
                </Show>
              </div>
            }
          >
            <div style={{
              display: "grid",
              "grid-template-columns": "repeat(2, minmax(0, 1fr))",
              gap: "8px",
            }}>
              <For each={summaryItems()}>{(item) => <IndustrySummaryTile item={item} />}</For>
            </div>
          </Show>
        </CardShell>
        <MarkdownTableCard
          tool={props.tool}
          badge="申万分类"
          detail={classificationTable()?.rows[0]?.[2] || subject}
          table={classificationTable()}
          callId={`${props.tool.call_id}:classification`}
          size="compact"
          rowsLimit={3}
          colsLimit={3}
          emptyText="暂无行业分类。"
        />
        <MarkdownTableCard
          tool={props.tool}
          badge="行业指数"
          detail={indexTable()?.rows.at(-1)?.[0]}
          table={indexTable()}
          callId={`${props.tool.call_id}:index`}
          size="normal"
          rowsLimit={8}
          colsLimit={5}
          emptyText="暂无行业指数序列。"
        />
        <MarkdownTableCard
          tool={props.tool}
          badge="同业样本"
          detail={peersTable()?.rows.length ? `${peersTable()?.rows.length} 家` : undefined}
          table={peersTable()}
          callId={`${props.tool.call_id}:peers`}
          size="normal"
          rowsLimit={12}
          colsLimit={2}
          emptyText="暂无同业样本。"
        />
        <MarkdownTableCard
          tool={props.tool}
          badge="行业资金面"
          detail={flowUnavailable() ? "来源缺口" : flowTable()?.rows[0]?.[0]}
          table={flowTable()}
          callId={`${props.tool.call_id}:flow`}
          size="wide"
          rowsLimit={3}
          colsLimit={9}
          emptyText="暂无行业资金面。"
        />
      </>
    </Show>
  );
}

function MarkdownTableCard(props: {
  tool: ToolCallView;
  badge: string;
  detail?: string;
  table?: MarkdownTable;
  callId: string;
  size?: "compact" | "normal" | "wide" | "hero";
  rowsLimit?: number;
  colsLimit?: number;
  emptyText: string;
}) {
  return (
    <CardShell
      status={props.tool.status}
      badge={props.badge}
      detail={tableIsEffectivelyEmpty(props.table) ? undefined : props.detail}
      tool={props.tool}
      callId={props.callId}
      size={props.size ?? "normal"}
    >
      <Show
        when={!tableIsEffectivelyEmpty(props.table)}
        fallback={<EmptyHint text={props.emptyText} />}
      >
        <FinancialTablePane table={props.table!} rowsLimit={props.rowsLimit} colsLimit={props.colsLimit} />
      </Show>
    </CardShell>
  );
}

function IndustrySummaryTile(props: { item: string }) {
  const [label, value] = splitSummaryItem(props.item);
  return (
    <div style={{
      padding: "8px 9px",
      background: "rgba(255,255,255,0.025)",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{
        "font-family": "var(--font-mono)",
        "font-size": "11px",
        color: "var(--ink-3)",
        "margin-bottom": "4px",
      }}>
        {label}
      </div>
      <div style={{
        color: "var(--ink-0)",
        "font-size": "13px",
        "line-height": 1.45,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        display: "-webkit-box",
        "-webkit-line-clamp": "3",
        "-webkit-box-orient": "vertical",
      }}>
        {value}
      </div>
    </div>
  );
}

function markdownSectionBullets(md: string, sectionTitle: string): string[] {
  const out: string[] = [];
  let active = false;
  for (const line of md.split("\n")) {
    const heading = line.match(/^#{2,3}\s+(.+)$/);
    if (heading) {
      active = heading[1].includes(sectionTitle);
      continue;
    }
    if (active && line.trim().startsWith("- ")) out.push(line.trim().replace(/^-\s+/, ""));
  }
  return out;
}

function splitSummaryItem(item: string): [string, string] {
  const idx = item.indexOf("：") >= 0 ? item.indexOf("：") : item.indexOf(":");
  if (idx < 0) return ["摘要", item];
  return [item.slice(0, idx).trim(), item.slice(idx + 1).trim()];
}

function findMarkdownTable(tables: MarkdownTable[], needles: string[]): MarkdownTable | undefined {
  return tables.find((table) =>
    needles.some((needle) => table.title.toLowerCase().includes(needle.toLowerCase()))
  );
}

/* ---------- get_capital_flow ---------- */

function numericCell(raw: string): number {
  const n = parseFloat(raw.replace(/[,，%]/g, ""));
  return Number.isFinite(n) ? n : 0;
}

function CapitalFlowCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tsCode = String(args.ts_code ?? "");
  const displayLabel = createMemo(() => securityLabel(toolOutput(props.tool)) || tsCode);
  const tables = createMemo(() => parseMarkdownTables(toolOutput(props.tool)));
  const stockTable = () => tables().find((t) => t.headers.some((h) => h.includes("净流入额")));
  const northTable = () => tables().find((t) => t.headers.some((h) => h.includes("北向")));
  const genericTables = () => tables().filter((table) => table !== stockTable() && table !== northTable() && table.rows.length > 0);
  const recentStock = () => stockTable()?.rows.slice(-7) ?? [];
  const recentNorth = () => northTable()?.rows.slice(-5) ?? [];
  const hasStockRows = () => recentStock().length > 0;
  const hasNorthRows = () => recentNorth().length > 0;
  const hasGenericRows = () => genericTables().length > 0;
  const northUnavailable = () => capitalFlowNorthboundUnavailable(toolOutput(props.tool));
  const paneCount = () => [hasStockRows(), hasNorthRows(), hasGenericRows(), northUnavailable()].filter(Boolean).length;
  const stockNetSum = () => recentStock().reduce((sum, row) => sum + numericCell(row[row.length - 1] ?? "0"), 0);
  const northNetSum = () => recentNorth().reduce((sum, row) => sum + numericCell(row[1] ?? "0"), 0);

  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={displayLabel() || "northbound"}
      tool={props.tool}
      callId={props.tool.call_id}
      size="wide"
    >
      <Show
        when={hasStockRows() || hasNorthRows() || hasGenericRows() || northUnavailable()}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No capital-flow data.">
              Fetching capital flow…
            </Show>
          </div>
        }
      >
        <div style={{
          display: "grid",
          "grid-template-columns": paneCount() > 1 ? "repeat(2, minmax(0, 1fr))" : "minmax(0, 1fr)",
          gap: "12px",
        }}>
          <Show when={hasStockRows()}>
            <FlowPane title={stockTable()?.title || `${displayLabel()} 资金流向`} unit="万元" rows={recentStock()} net={stockNetSum()} netIndex={5} />
          </Show>
          <Show when={hasNorthRows()}>
            <FlowPane title={northTable()?.title || "北向资金"} unit="亿" rows={recentNorth()} net={northNetSum()} netIndex={1} />
          </Show>
          <For each={genericTables().slice(0, 6)}>{(table) => (
            <FinancialTablePane table={table} rowsLimit={6} colsLimit={6} />
          )}</For>
          <Show when={northUnavailable() && !hasNorthRows()}>
            <FlowNoticePane />
          </Show>
        </div>
      </Show>
    </CardShell>
  );
}

function capitalFlowNorthboundUnavailable(md: string): boolean {
  return md.includes("no northbound data available") || md.includes("northbound disclosure unavailable");
}

function FlowNoticePane() {
  return (
    <div style={{
      padding: "10px 11px",
      background: "rgba(217,119,87,0.045)",
      border: "1px solid rgba(217,119,87,0.18)",
      "border-radius": "7px",
      color: "var(--ink-2)",
      "font-size": "12.5px",
      "line-height": 1.55,
    }}>
      <div style={{ color: "var(--ink-0)", "font-weight": 700, "margin-bottom": "4px" }}>
        北向资金口径不可用
      </div>
      <div>
        沪深港通披露机制调整后，近期北向净买入/净流入不能当作稳定资金面证据。不要把空值或零值解释为零流入，后续应改用个股资金流、成交活跃度、两融、行业资金或持股变化。
      </div>
    </div>
  );
}

// Capital-flow amounts arrive in 万元 (个股 net flow) or 亿 (北向). Render them in
// finance style: thousands separators, and a unit that adapts to magnitude — 万
// rolls up to 亿 once the value crosses 1 亿 (= 10,000 万) so large figures stay
// readable, and each row picks its own unit independently.
function formatFlowAmount(value: number, inputUnit: string): { text: string; unit: string } {
  const wan = inputUnit.includes("亿") ? value * 10000 : value;
  const abs = Math.abs(wan);
  const sign = wan > 0 ? "+" : wan < 0 ? "−" : "";
  const grouped = (n: number, digits: number) =>
    n.toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits });
  if (abs >= 10000) return { text: `${sign}${grouped(abs / 10000, 2)}`, unit: "亿" };
  return { text: `${sign}${grouped(abs, 1)}`, unit: "万" };
}

function FlowPane(props: {
  title: string;
  unit: string;
  rows: string[][];
  net: number;
  netIndex: number;
}) {
  const maxAbs = () => Math.max(1, ...props.rows.map((row) => Math.abs(numericCell(row[props.netIndex] ?? "0"))));
  const netColor = () => props.net >= 0 ? "var(--up)" : "var(--down)";
  const netAmount = () => formatFlowAmount(props.net, props.unit);
  return (
    <div style={{
      padding: "9px 10px",
      background: "rgba(255,255,255,0.02)",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{ display: "flex", "align-items": "baseline", gap: "8px", "margin-bottom": "8px" }}>
        <div style={{ "font-size": "13.5px", color: "var(--ink-0)", "font-weight": 650, overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
          {props.title}
        </div>
        <div style={{ "margin-left": "auto", "font-family": "var(--font-mono)", "font-size": "11px", color: netColor(), "font-weight": 700, "white-space": "nowrap" }}>
          {netAmount().text} {netAmount().unit}
        </div>
      </div>
      <div style={{ display: "flex", "flex-direction": "column", gap: "5px" }}>
        <For each={props.rows}>{(row) => {
          const net = numericCell(row[props.netIndex] ?? "0");
          const positive = net >= 0;
          // Diverging bar: a fixed centre axis with inflow growing right and
          // outflow growing left, so magnitude reads on a shared baseline.
          const halfWidth = Math.max(1.5, Math.min(50, Math.abs(net) / maxAbs() * 50));
          const color = positive ? "var(--up)" : "var(--down)";
          const amount = formatFlowAmount(net, props.unit);
          return (
            <div style={{ display: "grid", "grid-template-columns": "60px minmax(0, 1fr) 104px", gap: "8px", "align-items": "center" }}>
              <span style={{ "font-family": "var(--font-mono)", "font-size": "11px", color: "var(--ink-3)" }}>{row[0]}</span>
              <span style={{ position: "relative", height: "8px", background: "rgba(255,255,255,0.03)", "border-radius": "99px" }}>
                <span style={{ position: "absolute", left: "calc(50% - 0.5px)", top: "-1px", bottom: "-1px", width: "1px", background: "rgba(255,255,255,0.22)" }} />
                <span style={{
                  position: "absolute",
                  top: 0,
                  height: "100%",
                  width: `${halfWidth}%`,
                  ...(positive ? { left: "50%" } : { right: "50%" }),
                  background: color,
                  opacity: 0.8,
                  "border-radius": positive ? "0 99px 99px 0" : "99px 0 0 99px",
                }} />
              </span>
              <span style={{ "font-family": "var(--font-mono)", "font-size": "11px", color, "text-align": "right", "white-space": "nowrap" }}>
                {amount.text} {amount.unit}
              </span>
            </div>
          );
        }}</For>
      </div>
    </div>
  );
}

/* ---------- get_a_share_market_snapshot ---------- */

function AShareMarketSnapshotCard(props: { tool: ToolCallView }) {
  const tables = createMemo(() => parseMarkdownTables(toolOutput(props.tool)));
  const table = createMemo(() => tables()[0]);
  const extraTables = () => tables().slice(1);
  const args = parseArgs(props.tool);
  const codes = Array.isArray(args.ts_codes) ? args.ts_codes.join(", ") : "";
  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={table()?.rows.length ? `${table()?.rows.length}只标的` : codes}
      tool={props.tool}
      callId={props.tool.call_id}
      size="wide"
    >
      <Show
        when={table()?.rows.length}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No market snapshot.">
              Fetching market snapshot…
            </Show>
          </div>
        }
      >
        <div style={{
          overflow: "auto",
          border: "1px solid var(--line-1)",
          "border-radius": "7px",
        }}>
          <MarketSnapshotGrid table={table()!} />
        </div>
        <div style={{ "margin-top": "8px", "font-size": "11.5px", color: "var(--ink-3)" }}>
          {marketSnapshotFootnote(table()!)}
        </div>
        <Show when={extraTables().length > 0}>
          <div style={{ display: "grid", "grid-template-columns": "minmax(0, 1fr)", gap: "10px", "margin-top": "10px" }}>
            <For each={extraTables()}>{(extra) => (
              <FinancialTablePane table={extra} rowsLimit={6} colsLimit={8} />
            )}</For>
          </div>
        </Show>
      </Show>
    </CardShell>
  );
}

interface MarketSnapshotColumn {
  label: string;
  index: number;
  align?: "left" | "right";
  strong?: boolean;
  mono?: boolean;
  pct?: boolean;
}

const MARKET_SNAPSHOT_COLUMNS: Array<{ label: string; hints: string[]; align?: "left" | "right"; strong?: boolean; mono?: boolean; pct?: boolean }> = [
  { label: "代码", hints: ["代码", "code"], mono: true, strong: true },
  { label: "名称", hints: ["名称", "name"] },
  { label: "时间", hints: ["更新时间", "交易日", "trade_date"], align: "right", mono: true },
  { label: "最新价", hints: ["最新价", "最近收盘", "latest_close", "close"], align: "right", mono: true, strong: true },
  { label: "涨跌幅", hints: ["涨跌幅", "%chg"], align: "right", mono: true, pct: true },
  { label: "涨跌额", hints: ["涨跌额"], align: "right", mono: true },
  { label: "今开", hints: ["今开"], align: "right", mono: true },
  { label: "最高", hints: ["最高"], align: "right", mono: true },
  { label: "最低", hints: ["最低"], align: "right", mono: true },
  { label: "昨收", hints: ["昨收"], align: "right", mono: true },
  { label: "成交量", hints: ["成交量", "volume", "vol"], align: "right", mono: true },
  { label: "成交额", hints: ["成交额", "amount"], align: "right", mono: true },
  { label: "换手率", hints: ["换手", "turnover"], align: "right", mono: true },
  { label: "PE", hints: ["PE"], align: "right", mono: true },
  { label: "PB", hints: ["PB"], align: "right", mono: true },
  { label: "涨跌停", hints: ["涨跌停", "limit"], align: "right", mono: true },
];

function marketSnapshotColumns(table: MarkdownTable): MarketSnapshotColumn[] {
  const used = new Set<number>();
  const columns: MarketSnapshotColumn[] = [];
  for (const spec of MARKET_SNAPSHOT_COLUMNS) {
    const index = table.headers.findIndex((header, i) =>
      !used.has(i) && spec.hints.some((hint) => header.toLowerCase().includes(hint.toLowerCase()))
    );
    if (index < 0) continue;
    used.add(index);
    columns.push({ ...spec, index });
  }
  return columns;
}

function MarketSnapshotGrid(props: { table: MarkdownTable }) {
  const columns = createMemo(() => marketSnapshotColumns(props.table));
  const template = () => `minmax(92px,0.9fr) minmax(110px,1fr) ${"minmax(82px,0.75fr) ".repeat(Math.max(0, columns().length - 2)).trim()}`;
  return (
    <div style={{
      display: "grid",
      "grid-template-columns": template(),
      "min-width": `${Math.max(720, columns().length * 86)}px`,
      background: "rgba(255,255,255,0.018)",
    }}>
      <For each={columns()}>{(column) => (
        <QuoteHeader text={column.label} align={column.align} />
      )}</For>
      <For each={props.table.rows}>{(row) => {
        const pctColumn = columns().find((column) => column.pct);
        const pct = parseFloat(pctColumn ? row[pctColumn.index] ?? "" : "");
        const pctColor = Number.isFinite(pct) ? pct >= 0 ? "var(--up)" : "var(--down)" : "var(--ink-3)";
        return (
          <>
            <For each={columns()}>{(column) => {
              const value = row[column.index] || "—";
              const color = column.pct ? pctColor : undefined;
              const suffix = column.pct && value !== "—" && !value.endsWith("%") ? "%" : "";
              return (
                <QuoteCell mono={column.mono} strong={column.strong} align={column.align} color={color}>
                  {value}{suffix}
                </QuoteCell>
              );
            }}</For>
          </>
        );
      }}</For>
    </div>
  );
}

function marketSnapshotFootnote(table: MarkdownTable): string {
  const headers = table.headers.join(" ");
  if (headers.includes("最新价") || headers.includes("更新时间")) {
    return "盘中公开行情快照 · 成交量/成交额为当前快照口径 · 不与最近收盘口径混用";
  }
  return "最近交易日收盘快照 · 成交量/成交额为日线口径";
}

/* ---------- get_a_share_research_sources ---------- */

interface ResearchSourceSection {
  title: string;
  api: string;
  status: string;
  rows: string[];
  unavailable: boolean;
}

function parseResearchSourceSections(md: string): ResearchSourceSection[] {
  const sections: ResearchSourceSection[] = [];
  let current: ResearchSourceSection | null = null;
  for (const line of md.split("\n")) {
    const heading = line.match(/^###\s+(.+?)\s+·\s+`([^`]+)`\s+·\s+(.+)$/);
    if (heading) {
      current = {
        title: heading[1].trim(),
        api: heading[2].trim(),
        status: heading[3].trim(),
        rows: [],
        unavailable: heading[3].includes("unavailable") || heading[3].includes("不可用"),
      };
      sections.push(current);
      continue;
    }
    if (current && line.startsWith("- ")) current.rows.push(line.slice(2).trim());
  }
  return sections;
}

function AShareResearchSourcesCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tsCode = String(args.ts_code ?? "");
  const sections = createMemo(() => parseResearchSourceSections(toolOutput(props.tool)));
  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={tsCode || "structured sources"}
      tool={props.tool}
      callId={props.tool.call_id}
      size="wide"
    >
      <Show
        when={sections().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No structured source sections.">
              Fetching source material…
            </Show>
          </div>
        }
      >
        <div style={{
          display: "grid",
          "grid-template-columns": "repeat(auto-fit, minmax(240px, 1fr))",
          gap: "10px",
        }}>
          <For each={sections()}>{(section) => (
            <ResearchSourceSectionTile section={section} />
          )}</For>
        </div>
      </Show>
    </CardShell>
  );
}

function ResearchSourceSectionTile(props: { section: ResearchSourceSection }) {
  return (
    <div style={{
      padding: "9px 10px",
      background: props.section.unavailable ? "rgba(217,112,112,0.055)" : "rgba(255,255,255,0.025)",
      border: props.section.unavailable ? "1px solid rgba(217,112,112,0.22)" : "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{ display: "flex", gap: "8px", "align-items": "baseline", "margin-bottom": "7px" }}>
        <div style={{ color: "var(--ink-0)", "font-weight": 650, "font-size": "13px" }}>{props.section.title}</div>
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "10.5px",
          color: props.section.unavailable ? "#d97070" : "var(--ink-3)",
          "margin-left": "auto",
        }}>
          {props.section.status}
        </div>
      </div>
      <div style={{ display: "flex", "flex-direction": "column", gap: "5px" }}>
        <For each={props.section.rows.slice(0, 4)}>{(row) => (
          <ResearchSourceRow row={row} unavailable={props.section.unavailable} />
        )}</For>
      </div>
    </div>
  );
}

function ResearchSourceRow(props: { row: string; unavailable: boolean }) {
  const url = () => props.row.match(/https?:\/\/\S+/)?.[0] ?? "";
  const text = () => props.row.replace(/https?:\/\/\S+/g, "").replace(/\s+·\s*$/g, "").trim();
  return (
    <div style={{
      display: "flex",
      gap: "7px",
      "align-items": "center",
      "min-width": 0,
      color: props.unavailable ? "#d97070" : "var(--ink-2)",
      "font-size": "11px",
      "line-height": 1.35,
    }}>
      <span style={{
        flex: 1,
        "min-width": 0,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>{text()}</span>
      <Show when={url()}>
        <a
          href={url()}
          target="_blank"
          rel="noopener noreferrer"
          onClick={(e) => e.stopPropagation()}
          style={{
            color: "var(--clay-soft)",
            "font-family": "var(--font-mono)",
            "font-size": "10.5px",
            "text-decoration": "none",
            "flex-shrink": 0,
          }}
        >
          来源
        </a>
      </Show>
    </div>
  );
}

/* ---------- market_quote · multi-ticker quote chips ---------- */

interface TVQuoteRow {
  symbol: string;
  name: string;
  close: string;
  pct: string;
  abs: string;
  vol: string;
  cap: string;
  tech: string;
}

/** Parse the markdown table tradingview emits — each row is one ticker
 *  snapshot. Returns the rows in DOM order. */
function parseTradingViewTable(md: string): TVQuoteRow[] {
  const out: TVQuoteRow[] = [];
  const lines = md.split("\n");
  for (const line of lines) {
    if (!line.startsWith("|") || line.startsWith("|---") || line.includes("symbol")) continue;
    const cells = line.split("|").map((c) => c.trim()).filter((_, i, arr) => i > 0 && i < arr.length - 1);
    if (cells.length < 8) continue;
    const [symbol, name, close, pct, abs, vol, cap, tech] = cells;
    if (!symbol || symbol === "—") continue;
    out.push({ symbol, name, close, pct, abs, vol, cap, tech });
  }
  return out;
}

function MarketQuoteCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tickers = Array.isArray(args.tickers) ? (args.tickers as string[]) : [];
  const rows = createMemo(() => parseTradingViewTable(toolOutput(props.tool)));
  const multi = () => rows().length > 1;
  const detail = () => multi() ? `${rows().length}个标的` : tickers.join(", ");

  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={detail()}
      tool={props.tool}
      callId={props.tool.call_id}
      size={multi() ? "wide" : "normal"}
    >
      <Show
        when={rows().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No quotes.">
              Fetching quotes…
            </Show>
          </div>
        }
      >
        <Show
          when={multi()}
          fallback={<QuoteChip row={rows()[0]} />}
        >
          <QuoteMatrix rows={rows()} />
        </Show>
      </Show>
    </CardShell>
  );
}

function QuoteMatrix(props: { rows: TVQuoteRow[] }) {
  return (
    <div style={{
      overflow: "auto",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
    }}>
      <div style={{
        display: "grid",
        "grid-template-columns": "minmax(110px, 1.1fr) minmax(220px, 2fr) minmax(82px, 0.8fr) minmax(96px, 0.95fr) minmax(76px, 0.75fr) minmax(88px, 0.85fr)",
        gap: "0",
        "min-width": "720px",
        background: "rgba(255,255,255,0.018)",
      }}>
        <QuoteHeader text="symbol" />
        <QuoteHeader text="name" />
        <QuoteHeader text="price" align="right" />
        <QuoteHeader text="change" align="right" />
        <QuoteHeader text="volume" align="right" />
        <QuoteHeader text="mkt cap" align="right" />
        <For each={props.rows}>{(r) => <QuoteMatrixRow row={r} />}</For>
      </div>
    </div>
  );
}

function QuoteHeader(props: { text: string; align?: "left" | "right" }) {
  return (
    <div style={{
      padding: "7px 9px",
      color: "var(--ink-3)",
      "font-family": "var(--font-mono)",
      "font-size": "9px",
      "text-transform": "uppercase",
      "letter-spacing": "0.06em",
      "border-bottom": "1px solid rgba(255,255,255,0.055)",
      "text-align": props.align ?? "left",
    }}>
      {props.text}
    </div>
  );
}

function QuoteMatrixRow(props: { row: TVQuoteRow }) {
  const pctNum = parseFloat(props.row.pct);
  const up = !Number.isFinite(pctNum) ? null : pctNum >= 0;
  const color = up == null ? "var(--ink-3)" : up ? "var(--up)" : "var(--down)";
  return (
    <>
      <QuoteCell mono strong>{props.row.symbol}</QuoteCell>
      <QuoteCell>{props.row.name && props.row.name !== "—" ? props.row.name : "—"}</QuoteCell>
      <QuoteCell mono strong align="right">{props.row.close}</QuoteCell>
      <QuoteCell mono align="right" color={color}>
        {props.row.pct}% <span style={{ opacity: 0.72 }}>{props.row.abs}</span>
      </QuoteCell>
      <QuoteCell mono align="right">{props.row.vol || "—"}</QuoteCell>
      <QuoteCell mono align="right">{props.row.cap || "—"}</QuoteCell>
    </>
  );
}

function QuoteCell(props: {
  children: any;
  align?: "left" | "right";
  color?: string;
  mono?: boolean;
  strong?: boolean;
}) {
  return (
    <div style={{
      padding: "8px 9px",
      color: props.color ?? "var(--ink-1)",
      "font-family": props.mono ? "var(--font-mono)" : "inherit",
      "font-size": props.mono ? "10.5px" : "11px",
      "font-weight": props.strong ? 740 : 500,
      "line-height": 1.35,
      "border-bottom": "1px solid rgba(255,255,255,0.045)",
      "text-align": props.align ?? "left",
      "min-width": 0,
      overflow: "hidden",
      "text-overflow": "ellipsis",
      "white-space": "nowrap",
    }}>
      {props.children}
    </div>
  );
}

function QuoteChip(props: { row: TVQuoteRow }) {
  const pctNum = parseFloat(props.row.pct);
  const up = !Number.isFinite(pctNum) ? null : pctNum >= 0;
  const color = up == null ? "var(--ink-3)" : up ? "var(--up)" : "var(--down)";
  return (
    <div style={{
      display: "flex",
      "flex-direction": "column",
      gap: "10px",
      padding: "12px 13px",
      background: "rgba(255, 255, 255, 0.025)",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{ display: "grid", "grid-template-columns": "minmax(0, 1fr) auto", gap: "14px", "align-items": "start" }}>
        <div style={{ "min-width": 0 }}>
          <div style={{
            "font-family": "var(--font-mono)",
            "font-size": "13px",
            color: "var(--ink-0)",
            "font-weight": 750,
            "line-height": 1.2,
          }}>
            {props.row.symbol}
          </div>
          <div style={{
            "font-size": "13px",
            color: "var(--ink-2)",
            "line-height": 1.4,
            "margin-top": "5px",
            "overflow-wrap": "anywhere",
          }}>
            {props.row.name && props.row.name !== "—" ? props.row.name : "—"}
          </div>
        </div>
        <div style={{ "text-align": "right", "font-family": "var(--font-mono)" }}>
          <div style={{ color: "var(--ink-0)", "font-size": "22px", "font-weight": 760, "line-height": 1 }}>
            {props.row.close}
          </div>
          <div style={{ color, "font-size": "13px", "font-weight": 750, "margin-top": "7px", "white-space": "nowrap" }}>
            {props.row.pct}% <span style={{ color, opacity: 0.78 }}>{props.row.abs}</span>
          </div>
        </div>
      </div>
      <div style={{ display: "grid", "grid-template-columns": "repeat(3, minmax(0, 1fr))", gap: "8px" }}>
        <QuoteStat label="Volume" value={props.row.vol} />
        <QuoteStat label="Mkt cap" value={props.row.cap} />
        <QuoteStat label="Tech" value={props.row.tech} />
      </div>
    </div>
  );
}

function QuoteStat(props: { label: string; value: string }) {
  return (
    <div style={{
      padding: "7px 8px",
      background: "rgba(255,255,255,0.025)",
      border: "1px solid rgba(255,255,255,0.07)",
      "border-radius": "6px",
      "min-width": 0,
    }}>
      <div style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)", "margin-bottom": "3px" }}>
        {props.label}
      </div>
      <div style={{
        "font-family": "var(--font-mono)",
        "font-size": "12.5px",
        color: "var(--ink-1)",
        "font-weight": 650,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>
        {props.value || "—"}
      </div>
    </div>
  );
}

/* ---------- candlesticks ---------- */

interface OhlcvRow {
  date: string;
  open: number;
  high: number;
  low: number;
  close: number;
  pctChg: number;
  vol: number;
}

function parseMarketNumber(raw: string): number {
  const trimmed = raw.trim();
  if (!trimmed || trimmed === "—") return NaN;
  const suffix = trimmed.match(/[KMB]$/i)?.[0]?.toUpperCase();
  const base = parseFloat(trimmed.replace(/[,，%]/g, "").replace(/[KMB]$/i, ""));
  if (!Number.isFinite(base)) return NaN;
  if (suffix === "K") return base * 1_000;
  if (suffix === "M") return base * 1_000_000;
  if (suffix === "B") return base * 1_000_000_000;
  return base;
}

function parseCandlestickTable(md: string): OhlcvRow[] {
  const rows: OhlcvRow[] = [];
  for (const line of md.split("\n")) {
    if (!line.startsWith("|") || line.includes("---") || /Date|日期/i.test(line)) continue;
    const cells = line.split("|").map((c) => c.trim()).filter((_, i, arr) => i > 0 && i < arr.length - 1);
    if (cells.length < 6 || !/^\d{4}-\d{2}-\d{2}(?:\s+\d{2}:\d{2}(?::\d{2})?)?$/.test(cells[0])) continue;
    const open = parseMarketNumber(cells[1]);
    const high = parseMarketNumber(cells[2]);
    const low = parseMarketNumber(cells[3]);
    const close = parseMarketNumber(cells[4]);
    const hasPct = cells.length >= 7;
    const pctChg = hasPct ? parseMarketNumber(cells[5]) : (open ? ((close - open) / open) * 100 : 0);
    const vol = parseMarketNumber(hasPct ? cells[6] : cells[5]);
    if (![open, high, low, close].every(Number.isFinite)) continue;
    rows.push({
      date: cells[0],
      open,
      high,
      low,
      close,
      pctChg: Number.isFinite(pctChg) ? pctChg : 0,
      vol: Number.isFinite(vol) ? vol : 0,
    });
  }
  return rows;
}

function candlestickSummary(md: string): string {
  return md.match(/_摘要:\s*([^_]+)_/)?.[1]?.trim() ?? "";
}

function Candlestick(props: { data: OhlcvRow[]; height: number }) {
  let host!: HTMLDivElement;
  let chart: IChartApi | null = null;
  let series: ISeriesApi<"Candlestick"> | null = null;
  let ro: ResizeObserver | null = null;

  const hasIntraday = () => props.data.some((d) => d.date.includes(":"));
  const toChartTime = (value: string) => {
    if (!value.includes(":")) return value;
    const normalized = value.length === 16 ? `${value}:00` : value;
    return Math.floor(new Date(normalized.replace(" ", "T")).getTime() / 1000);
  };
  const toSeriesData = () =>
    props.data.map((d) => ({
      time: toChartTime(d.date) as any,
      open: d.open,
      high: d.high,
      low: d.low,
      close: d.close,
    }));
  const showLatestWindow = () => {
    if (!chart) return;
    const len = props.data.length;
    if (len <= 90) {
      chart.timeScale().fitContent();
      return;
    }
    chart.timeScale().setVisibleLogicalRange({
      from: Math.max(0, len - 90),
      to: len + 5,
    });
  };

  onMount(() => {
    if (!host) return;
    chart = createChart(host, {
      height: props.height,
      width: host.clientWidth || 560,
      autoSize: false,
      layout: {
        background: { color: "transparent" },
        textColor: "rgba(180, 165, 150, 0.7)",
        fontSize: 10,
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      },
      grid: {
        vertLines: { color: "rgba(255, 240, 220, 0.04)" },
        horzLines: { color: "rgba(255, 240, 220, 0.06)" },
      },
      timeScale: {
        timeVisible: hasIntraday(),
        borderColor: "rgba(255, 240, 220, 0.08)",
        fixLeftEdge: false,
        fixRightEdge: false,
        rightOffset: 6,
      },
      rightPriceScale: {
        borderColor: "rgba(255, 240, 220, 0.08)",
      },
      crosshair: {
        vertLine: { color: "rgba(217, 119, 87, 0.45)", width: 1, style: 0 },
        horzLine: { color: "rgba(217, 119, 87, 0.45)", width: 1, style: 0 },
      },
      handleScroll: true,
      handleScale: true,
    });
    // Read the resolved --up / --down tokens so the candles follow the user's
    // 涨跌配色 setting (default 红涨绿跌). lightweight-charts needs concrete hex.
    const cs = getComputedStyle(document.documentElement);
    const upHex = cs.getPropertyValue("--up").trim() || "#e35b4e";
    const downHex = cs.getPropertyValue("--down").trim() || "#5fb98f";
    series = chart.addSeries(CandlestickSeries, {
      upColor: upHex,
      downColor: downHex,
      borderVisible: false,
      wickUpColor: upHex,
      wickDownColor: downHex,
    });
    series.setData(toSeriesData());
    showLatestWindow();

    ro = new ResizeObserver(() => {
      if (chart && host) {
        chart.applyOptions({ width: host.clientWidth });
      }
    });
    ro.observe(host);
  });

  // Push new data through whenever the rows change (e.g. tool re-runs).
  createEffect(() => {
    if (!series) return;
    chart?.applyOptions({ timeScale: { timeVisible: hasIntraday() } });
    series.setData(toSeriesData());
    showLatestWindow();
  });

  onCleanup(() => {
    ro?.disconnect();
    chart?.remove();
    chart = null;
    series = null;
  });

  return <div ref={host} style={{ width: "100%", height: `${props.height}px` }} />;
}

function CandlestickToolCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const ticker = String(args.ticker ?? "");
  const market = String(args.market ?? "");
  const isAshare = market === "a_share" || market === "ashare" || market === "cn";
  const initialInterval = String(args.interval ?? "1d");
  const [interval, setInterval] = createSignal(isAshare && initialInterval === "15m" ? "1d" : initialInterval);
  const [preview, setPreview] = createSignal(toolOutput(props.tool));
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal("");
  const rows = createMemo(() => parseCandlestickTable(preview()));
  const summary = createMemo(() => candlestickSummary(preview()));
  const last = () => rows().length > 0 ? rows()[rows().length - 1] : null;
  const detail = () => {
    const label = securityLabel(preview()) || ticker;
    const r = last();
    if (!r) return [label, market].filter(Boolean).join(" · ");
    const sign = r.pctChg >= 0 ? "+" : "";
    return `${label} · ${r.close.toFixed(2)} (${sign}${r.pctChg.toFixed(2)}%)`;
  };
  const load = async (nextInterval: string) => {
    if (!ticker || !market) return;
    if (isAshare && nextInterval === "15m") {
      setError("A 股分钟线需要 pytdx 接入；当前 Tushare token 无 stk_mins 权限。");
      return;
    }
    setInterval(nextInterval);
    setLoading(true);
    setError("");
    const limit = nextInterval === "15m" ? 240 : 180;
    try {
      const url = new URL("/api/v1/tools/candlesticks", window.location.origin);
      url.searchParams.set("ticker", ticker);
      url.searchParams.set("market", market);
      url.searchParams.set("interval", nextInterval);
      url.searchParams.set("limit", String(limit));
      setPreview(await fetchToolPreview(url));
    } catch (e) {
      setError(parseCandlestickTable(preview()).length > 0 ? "" : e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  onMount(() => {
    if (props.tool.status === "completed" && ticker && market && parseCandlestickTable(preview()).length === 0) {
      void load(interval());
    }
  });

  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={detail()}
      tool={props.tool}
      callId={props.tool.call_id}
      size="wide"
    >
      <div style={{ display: "flex", "align-items": "center", gap: "8px", "margin-bottom": "10px" }}>
        <div style={{ display: "inline-flex", gap: "4px", padding: "2px", background: "rgba(255,255,255,0.03)", border: "1px solid var(--line-1)", "border-radius": "6px" }}>
          <For each={[
            ["15m", "15m"],
            ["1d", "日"],
            ["1w", "周"],
            ["1mo", "月"],
          ]}>{([value, label]) => {
            const disabled = isAshare && value === "15m";
            return (
              <button
                type="button"
                disabled={disabled}
                onClick={(e) => { e.stopPropagation(); void load(value); }}
                style={{
                  appearance: "none",
                  border: "0",
                  background: interval() === value ? "rgba(217,119,87,0.18)" : "transparent",
                  color: disabled ? "rgba(160,145,130,0.35)" : interval() === value ? "var(--clay-soft)" : "var(--ink-3)",
                  "font-family": "var(--font-mono)",
                  "font-size": "11.5px",
                  padding: "4px 8px",
                  "border-radius": "4px",
                  cursor: disabled ? "not-allowed" : "pointer",
                }}
              >{label}</button>
            );
          }}</For>
        </div>
        <Show when={isAshare}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "11.5px", color: "var(--ink-3)" }}>
            15m 等 pytdx 接入
          </span>
        </Show>
        <Show when={loading()}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "11.5px", color: "var(--ink-3)" }}>
            loading candles…
          </span>
        </Show>
        <Show when={error()}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "11.5px", color: "#d97070", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
            {error()}
          </span>
        </Show>
      </div>
      <Show
        when={rows().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No candles.">
              Fetching candlesticks…
            </Show>
          </div>
        }
      >
        <Show when={summary()}>
          <div style={{
            "font-family": "var(--font-mono)",
            "font-size": "11.5px",
            color: "var(--ink-2)",
            "line-height": 1.45,
            "margin-bottom": "8px",
          }}>
            {summary()}
          </div>
        </Show>
        <Candlestick data={rows()} height={260} />
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "11px",
          color: "var(--ink-3)",
          "margin-top": "8px",
        }}>
          K 线按表格保留窗口绘制；最新收盘以摘要和行情快照为准。
        </div>
      </Show>
    </CardShell>
  );
}

/* ---------- sec_filing_fetch ---------- */

interface SecFiling {
  form: string;
  filed: string;
  primary_doc_url: string;
  description?: string;
}

function SecFilingCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const ticker = String(args.ticker ?? "").toUpperCase();
  const preview = toolOutput(props.tool);
  const parsed = safeParseJson(preview) as
    | { filings?: SecFiling[]; company_name?: string } | null;
  const filings: SecFiling[] = parsed?.filings
    ?? (extractObjectsAfter(preview, "\"filings\"") as SecFiling[]);
  return (
    <CardShell
      status={props.tool.status}
      badge={toolDisplayName(props.tool.name)}
      detail={parsed?.company_name ? `${ticker} · ${parsed.company_name}` : ticker}
      tool={props.tool}
      callId={props.tool.call_id}
      size="normal"
    >
      <Show
        when={filings.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "12.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No filings.">
              Fetching filings…
            </Show>
          </div>
        }
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
          <For each={filings.slice(0, 5)}>{(f) => (
            <a
              href={f.primary_doc_url}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              style={{
                display: "flex",
                gap: "10px",
                padding: "6px 10px",
                background: "rgba(255, 255, 255, 0.025)",
                border: "1px solid var(--line-1)",
                "border-radius": "6px",
                "font-family": "var(--font-mono)",
                "font-size": "11px",
                color: "var(--ink-1)",
                "text-decoration": "none",
              }}
            >
              <span style={{ "min-width": "50px", color: "var(--clay-soft)" }}>{f.form}</span>
              <span style={{ "min-width": "100px", color: "var(--ink-3)" }}>{f.filed}</span>
              <span style={{ flex: 1, "white-space": "nowrap", overflow: "hidden", "text-overflow": "ellipsis" }}>
                {f.description || f.primary_doc_url.split("/").slice(-1)[0]}
              </span>
            </a>
          )}</For>
        </div>
      </Show>
    </CardShell>
  );
}

/* ---------- generic fallback ---------- */

function GenericToolCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const summary = (() => {
    if (typeof args.url === "string") return String(args.url);
    if (typeof args.query === "string") return String(args.query);
    if (typeof args.ts_code === "string") return String(args.ts_code);
    if (Array.isArray(args.tickers)) return (args.tickers as string[]).join(", ");
    return JSON.stringify(args).slice(0, 60);
  })();
  return (
    <CardShell status={props.tool.status} badge={toolDisplayName(props.tool.name)} detail={summary} tool={props.tool} callId={props.tool.call_id} size="compact">
      <Show when={toolOutput(props.tool)}>
        <div style={{
          "font-size": "12.5px",
          color: "var(--ink-2)",
          "line-height": 1.45,
          "max-height": "180px",
          overflow: "auto",
        }}>
          <SafeMarkdown source={toolOutput(props.tool)} />
        </div>
      </Show>
    </CardShell>
  );
}
