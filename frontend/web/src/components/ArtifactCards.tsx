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

export interface MandateCheckView {
  fits_mandate: boolean;
  notes: string;
}

export interface DecisionDraftView {
  /** Set by the backend so chat-panel can dedupe drafts within one task —
   *  critic-driven rewrites cause the same task to record_investment_action
   *  multiple times. */
  task_id?: string;
  deliverable_id: string;
  ticker: string;
  direction: string;
  size_pct?: number;
  stop_loss?: number;
  target?: number;
  horizon_days?: number;
  rationale: string;
  risks?: string[];
  opposing_case?: string;
  corpus_refs?: string[];
  mandate_check?: MandateCheckView;
  invalidation_conditions?: string;
}

export interface SubagentRunView {
  run_id: string;
  role: string;
  status: "in_progress" | "completed" | "error" | string;
  question: string;
  output_preview?: string;
  tokens_used?: number;
  duration_ms?: number;
  error?: string;
}

export interface ArtifactEventView {
  id: string;
  kind: "narration" | "narration_group" | "search" | "tool" | "decision" | "subagent";
  narration?: NarrationStep;
  narrations?: NarrationStep[];
  search?: SearchCallView;
  tool?: ToolCallView;
  decision?: DecisionDraftView;
  subagent?: SubagentRunView;
}

interface ArtifactCallbacks {
  onOpenDoc?: (id: string, title: string) => void;
}

export function ArtifactPanel(props: {
  searches: SearchCallView[];
  tools: ToolCallView[];
  narrations?: NarrationStep[];
  events?: ArtifactEventView[];
  callbacks?: ArtifactCallbacks;
}) {
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
        if (last?.kind === "narration_group") {
          last.narrations = [...(last.narrations ?? []), event.narration];
        } else {
          out.push({
            id: event.id,
            kind: "narration_group",
            narrations: [event.narration],
          });
        }
      } else {
        out.push(event);
      }
    }
    return out;
  });

  // Sticky-bottom auto-scroll: when new artifact cards land while the user
  // is already at (or near) the bottom of the canvas, follow the new card
  // into view. If the user has scrolled up to read earlier cards, do not
  // yank them back — that would make the canvas unusable for review.
  let gridEl: HTMLDivElement | undefined;
  let wasAtBottom = true;
  const updateStickiness = () => {
    if (!gridEl) return;
    const slack = 80; // px tolerance — user counts as "at bottom" within this band
    wasAtBottom = gridEl.scrollTop + gridEl.clientHeight + slack >= gridEl.scrollHeight;
  };
  createEffect(() => {
    // Re-run whenever the rendered card list changes.
    groupedEvents().length;
    if (gridEl && wasAtBottom) {
      // Defer to after Solid commits the new DOM.
      queueMicrotask(() => {
        if (gridEl) gridEl.scrollTop = gridEl.scrollHeight;
      });
    }
  });

  return (
    <div class="lk-artifact-grid" ref={(el) => { gridEl = el; }} onScroll={updateStickiness}>
      <For each={groupedEvents()}>{(event) => (
        <ArtifactEventCard event={event} callbacks={props.callbacks} />
      )}</For>
    </div>
  );
}

function normalizeNarrationText(text: string): string {
  return text.replace(/\s+/g, " ").trim();
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
    case "subagent":
      return props.event.subagent ? <SubagentArtifact id={props.event.id} run={props.event.subagent} /> : null;
  }
}

function SubagentArtifact(props: { id: string; run: SubagentRunView }) {
  const [expanded, setExpanded] = createSignal(false);
  // Live elapsed counter for the in_progress state. Without it, a
  // running subagent looks identical to a stalled one — the user can't
  // tell whether anything is happening. We start the counter the moment
  // the card mounts (which coincides with the in_progress event for live
  // sessions; for history replay the card is already completed and the
  // counter is hidden by the status check).
  const cardMountedAt = Date.now();
  const [liveSec, setLiveSec] = createSignal(0);
  let timer: number | undefined;
  createEffect(() => {
    const running = props.run.status === "in_progress";
    if (running && timer == null) {
      timer = window.setInterval(() => {
        setLiveSec(Math.floor((Date.now() - cardMountedAt) / 1000));
      }, 500);
    } else if (!running && timer != null) {
      clearInterval(timer);
      timer = undefined;
    }
  });
  onCleanup(() => { if (timer != null) clearInterval(timer); });

  const isRunning = () => props.run.status === "in_progress";
  const statusTone = () =>
    props.run.status === "completed" ? "#9eb78f" :
    props.run.status === "error"     ? "#d97070" : "#e8b86c";
  const statusLabel = () =>
    props.run.status === "completed" ? "已完成" :
    props.run.status === "error"     ? "出错" :
    "运行中";
  const preview = () => props.run.output_preview ?? "";
  const hasOutput = () => preview().length > 0 || props.run.error;
  return (
    <div
      data-call-id={props.id}
      class={`lk-artifact-card is-normal is-subagent${isRunning() ? " is-running" : ""}`}
      style={{
        background: isRunning()
          ? "linear-gradient(180deg, rgba(232,184,108,0.14), rgba(182,154,214,0.05))"
          : "linear-gradient(180deg, rgba(182,154,214,0.10), rgba(255,255,255,0.02))",
        border: isRunning()
          ? "1px solid rgba(232,184,108,0.45)"
          : "1px solid rgba(182,154,214,0.28)",
        "border-radius": "8px",
        padding: "12px 14px",
      }}
    >
      <div style={{ display: "flex", "align-items": "baseline", gap: "10px", "margin-bottom": "6px", "flex-wrap": "wrap" }}>
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "10px",
          color: "#b69ad6",
          "letter-spacing": "0.06em",
          "font-weight": 700,
        }}>
          子代理 · {subagentRoleLabel(props.run.role)}
        </span>
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "10px",
          color: statusTone(),
          "font-weight": 600,
        }}>
          <Show when={isRunning()}>
            <span class="lk-subagent-spinner" style={{ "margin-right": "4px" }}>⏳</span>
          </Show>
          {statusLabel()}
        </span>
        <Show when={isRunning()}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)" }}>
            {liveSec()}s
          </span>
        </Show>
        <Show when={!isRunning() && typeof props.run.duration_ms === "number"}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)" }}>
            {Math.max(1, Math.round((props.run.duration_ms ?? 0) / 100) / 10)}s
          </span>
        </Show>
      </div>
      <div style={{ "font-size": "12px", color: "var(--ink-1)", "margin-bottom": hasOutput() ? "8px" : 0, "line-height": 1.5 }}>
        {props.run.question}
      </div>
      <Show when={props.run.error}>
        {(err) => (
          <div style={{
            color: "#d97070",
            "font-family": "var(--font-mono)",
            "font-size": "11px",
            background: "rgba(217,112,112,0.08)",
            padding: "6px 8px",
            "border-radius": "4px",
          }}>
            {err()}
          </div>
        )}
      </Show>
      <Show when={preview()}>
        <div
          onClick={() => setExpanded(!expanded())}
          style={{
            "font-size": "12.5px",
            color: "var(--ink-1)",
            "line-height": 1.55,
            cursor: "pointer",
            "max-height": expanded() ? "none" : "9.5em",
            overflow: "hidden",
          }}
          title={expanded() ? "click to collapse" : "click to expand"}
        >
          <SafeMarkdown source={preview()} />
        </div>
      </Show>
    </div>
  );
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
          "font-size": "10.5px",
          color: "var(--clay-soft)",
          "font-weight": 700,
          "text-transform": "uppercase",
          "letter-spacing": "0.08em",
        }}>
          reasoning trace
        </span>
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "10px",
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
              "font-size": "10px",
              "font-weight": 700,
            }}>
              {String(step.seq ?? index() + 1).padStart(2, "0")}
            </div>
            <div style={{
              color: "var(--ink-1)",
              "font-size": "12.5px",
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
    props.draft.direction === "long" ? "#d97757"
    : props.draft.direction === "short" ? "#6fb98a"
    : "#e8b86c";
  const stat = (label: string, value: string | number | undefined) => (
    <Show when={value != null && value !== ""}>
      <div style={{
        padding: "7px 8px",
        background: "rgba(255,255,255,0.03)",
        border: "1px solid rgba(255,255,255,0.08)",
        "border-radius": "6px",
        "min-width": 0,
      }}>
        <div style={{ "font-family": "var(--font-mono)", "font-size": "9.5px", color: "var(--ink-3)", "margin-bottom": "3px" }}>
          {label}
        </div>
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "12px",
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
        <span style={{ color: directionTone(), "font-size": "12px", "font-weight": 800, "letter-spacing": "0.08em" }}>
          ACTION · {directionLabel()}
        </span>
        <span style={{ color: "var(--ink-2)", "font-size": "12px", "font-weight": 700 }}>
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
        "font-size": "12.5px",
        "line-height": 1.55,
        "margin-bottom": "10px",
        // The agent often writes rationale as markdown (bold thesis lines,
        // bullet sub-claims, inline links). Render it as markdown so users
        // don't see raw `**` / `##` characters.
      }}>
        <SafeMarkdown source={props.draft.rationale} />
      </div>
      <Show when={props.draft.risks && props.draft.risks.length > 0}>
        <DecisionSection title="风险" tone="risk">
          <ul style={{ margin: 0, "padding-left": "18px", display: "flex", "flex-direction": "column", gap: "3px" }}>
            <For each={props.draft.risks}>{(risk) => <li><SafeMarkdown source={risk} /></li>}</For>
          </ul>
        </DecisionSection>
      </Show>
      <Show when={props.draft.opposing_case}>
        <DecisionSection title="反方观点" tone="bear">
          <SafeMarkdown source={props.draft.opposing_case!} />
        </DecisionSection>
      </Show>
      <Show when={props.draft.invalidation_conditions}>
        <DecisionSection title="失效条件" tone="risk">
          <SafeMarkdown source={props.draft.invalidation_conditions!} />
        </DecisionSection>
      </Show>
      <Show when={props.draft.mandate_check}>
        {(check) => (
          <DecisionSection title={`Mandate · ${check().fits_mandate ? "符合" : "需复核"}`} tone={check().fits_mandate ? "ok" : "risk"}>
            <SafeMarkdown source={check().notes ?? ""} />
          </DecisionSection>
        )}
      </Show>
      <Show when={props.draft.corpus_refs && props.draft.corpus_refs.length > 0}>
        <DecisionSection title="Corpus 引用" tone="ref">
          <div style={{ display: "flex", "flex-wrap": "wrap", gap: "4px" }}>
            <For each={props.draft.corpus_refs}>
              {(ref) => (
                <span style={{
                  padding: "2px 7px",
                  border: "1px solid rgba(255,255,255,0.12)",
                  "border-radius": "4px",
                  "font-size": "10.5px",
                  "font-family": "var(--font-mono)",
                }}>
                  {ref}
                </span>
              )}
            </For>
          </div>
        </DecisionSection>
      </Show>
    </div>
  );
}

function DecisionSection(props: { title: string; tone: "risk" | "bear" | "ok" | "ref"; children: any }) {
  const accent = () => ({
    risk: "#d97070",
    bear: "#7a9bd6",
    ok: "#9eb78f",
    ref: "#c9a76a",
  })[props.tone];
  return (
    <div style={{ "margin-top": "8px" }}>
      <div style={{
        "font-family": "var(--font-mono)",
        "font-size": "9.5px",
        "letter-spacing": "0.08em",
        "text-transform": "uppercase",
        color: accent(),
        "margin-bottom": "4px",
      }}>
        {props.title}
      </div>
      <div style={{
        color: "var(--ink-1)",
        "font-size": "12px",
        "line-height": 1.5,
      }}>
        {props.children}
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

function CardShell(props: {
  status: string;
  badge: string;
  detail?: string;
  children: any;
  onClick?: () => void;
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
        gap: "0",
        "margin-bottom": "8px",
        "font-family": "var(--font-mono)",
        "font-size": "10.5px",
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
          }}>
            {props.detail}
          </span>
        </Show>
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
          onClick={(e) => e.stopPropagation()}
          style={{
            width: "min(1320px, 96vw)",
            height: "min(900px, 92vh)",
            background: "#050505",
            border: "1px solid rgba(255,255,255,0.13)",
            "border-radius": "10px",
            overflow: "hidden",
            display: "flex",
            "flex-direction": "column",
            "box-shadow": "0 30px 90px rgba(0,0,0,0.62)",
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
              <div style={{ "font-size": "12px", color: "var(--ink-3)", "font-family": "var(--font-mono)", "margin-bottom": "3px" }}>
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
    props.search.action === "open_page" ? "web · page" :
    props.search.action === "find_in_page" ? "web · find" :
    "web · search";
  const verb = () => props.search.status === "completed"
    ? props.search.action === "open_page" ? "Opened"
    : props.search.action === "find_in_page" ? "Found in page"
    : "Searched"
    : props.search.action === "open_page" ? "Opening"
    : "Searching";
  const primaryDetail = () => props.search.detail || props.search.queries?.[0] || "";
  // The provider sometimes emits a `web_search_call` `output_item.added`
  // event with `action=unknown` and no metadata (just a status flip), and
  // its matching `output_item.done` may carry the real action+detail OR
  // may simply close out without further data. When neither side has any
  // identifying content, the card is just a placeholder horizontal rule
  // ("—") on the canvas — visually noisy and tells the user nothing.
  // Skip rendering in that case.
  const hasContent = () =>
    !!primaryDetail()
    || (props.search.queries?.length ?? 0) > 0
    || (props.search.sources?.length ?? 0) > 0;
  if (!hasContent()) return null;
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
        "font-size": "10.5px",
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
              "font-size": "10.5px",
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
            "font-size": "10px",
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
                "font-size": "10.5px",
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
          "font-size": "10.5px",
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
    case "corpus_read":   return <CorpusReadCard {...props} />;
    case "web_fetch":     return <WebFetchCard {...props} />;
    case "market_quote":
    case "tradingview_quote": return <MarketQuoteCard {...props} />;
    case "get_candlesticks": return <CandlestickToolCard {...props} />;
    case "tushare_quote": return <TushareCard {...props} />;
    case "use_skill": return <UseSkillCard {...props} />;
    case "get_company_info": return <CompanyInfoCard {...props} />;
    case "get_financials": return <FinancialsCard {...props} />;
    case "get_capital_flow": return <CapitalFlowCard {...props} />;
    case "sec_filing_fetch": return <SecFilingCard {...props} />;
    // `delegate_research` is the agent tool call that spawns a subagent;
    // the matching `subagent_run` event already renders a richer card
    // (with role label, status, output preview). Showing both creates
    // duplicate noise on the canvas, so suppress the tool-call card here.
    case "delegate_research": return null;
    default:              return <GenericToolCard {...props} />;
  }
}

/// Subagent role → user-facing label. Roles come from the backend
/// `delegate_research` tool's hard-coded persona list (fundamental_analyst,
/// trading_analyst, risk_manager, corpus_guardian — see
/// `crates/gateway/src/agent/tools/delegate_research.rs::role_instruction`).
function subagentRoleLabel(role: string): string {
  switch (role) {
    case "risk_manager": return "风控代理";
    case "fundamental_analyst": return "基本面分析";
    case "trading_analyst": return "交易分析";
    case "corpus_guardian": return "知识审核";
    default: return role;
  }
}

function CorpusReadCard(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  const args = parseArgs(props.tool);
  const id = String(args.id ?? "");
  const preview = props.tool.output_preview ?? "";
  const parsed = parseCorpusReadPreview(preview);
  const title = parsed?.title ?? id;
  const tierLayer = parsed ? [parsed.tier, parsed.layer].filter(Boolean).join("·") : "";
  const body = (parsed?.body ?? "").trim();
  const bodyPreview = () => {
    if (!body) return "";
    if (body.length <= 1200) return body;
    return body.slice(0, 1200).trimEnd() + "…";
  };
  return (
    <CardShell
      status={props.tool.status}
      badge="corpus·read"
      detail={title}
      callId={props.tool.call_id}
      size="normal"
    >
      <Show
        when={body.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No body returned.">
              Reading the corpus document…
            </Show>
          </div>
        }
      >
        <Show when={tierLayer.trim().length > 0}>
          <div
            style={{
              "font-family": "var(--font-mono)",
              "font-size": "10px",
              color: "var(--ink-3)",
              "margin-bottom": "6px",
            }}
          >
            {tierLayer}
          </div>
        </Show>
        <div
          onClick={(e) => {
            e.stopPropagation();
            props.callbacks?.onOpenDoc?.(parsed?.id ?? id, title);
          }}
          style={{
            cursor: "pointer",
            "font-size": "12px",
            "line-height": "1.55",
            color: "var(--ink-1)",
            "max-height": "260px",
            overflow: "hidden",
          }}
        >
          <SafeMarkdown source={bodyPreview()} />
        </div>
        <Show when={parsed?.truncated}>
          <div
            style={{
              "font-family": "var(--font-mono)",
              "font-size": "10px",
              color: "var(--ink-3)",
              "margin-top": "6px",
            }}
          >
            … truncated, click to open the full document
          </div>
        </Show>
      </Show>
    </CardShell>
  );
}

type CorpusReadPreview = {
  id?: string;
  title?: string;
  tier?: string;
  layer?: string;
  body?: string;
  truncated?: boolean;
};

function parseCorpusReadPreview(preview: string): CorpusReadPreview | null {
  const parsed = parseCorpusReadPayload(preview);
  if (parsed?.body) return parsed;

  // `output_preview` is intentionally capped and often cuts inside the large
  // JSON string returned by corpus_read. Salvage the body field so the card
  // can still show the document excerpt instead of a false "No body" state.
  const body = extractJsonStringField(preview, "body");
  if (!body) return parsed;
  return { ...(parsed ?? {}), body, truncated: true };
}

function parseCorpusReadPayload(s: string | undefined): CorpusReadPreview | null {
  const parsed = safeParseJson(s);
  if (isCorpusReadPayload(parsed)) return parsed;
  if (parsed && typeof parsed === "object" && typeof (parsed as { output?: unknown }).output === "string") {
    const nested = safeParseJson((parsed as { output: string }).output);
    if (isCorpusReadPayload(nested)) return nested;
  }
  return null;
}

function isCorpusReadPayload(v: unknown): v is CorpusReadPreview {
  return !!v && typeof v === "object" && (
    typeof (v as CorpusReadPreview).body === "string" ||
    typeof (v as CorpusReadPreview).title === "string" ||
    typeof (v as CorpusReadPreview).id === "string"
  );
}

function extractJsonStringField(source: string | undefined, key: string): string {
  if (!source) return "";
  const needle = `"${key}":"`;
  const start = source.indexOf(needle);
  if (start < 0) return "";

  let escaped = "";
  let escaping = false;
  for (let i = start + needle.length; i < source.length; i++) {
    const ch = source[i];
    if (!escaping && ch === "\"") break;
    escaped += ch;
    if (escaping) escaping = false;
    else if (ch === "\\") escaping = true;
  }

  escaped = escaped.replace(/…$/, "");
  while (hasOddTrailingBackslashes(escaped)) {
    escaped = escaped.slice(0, -1);
  }

  try {
    return JSON.parse(`"${escaped}"`);
  } catch {
    return escaped
      .replace(/\\n/g, "\n")
      .replace(/\\"/g, "\"")
      .replace(/\\\\/g, "\\");
  }
}

function hasOddTrailingBackslashes(s: string): boolean {
  let count = 0;
  for (let i = s.length - 1; i >= 0 && s[i] === "\\"; i--) count++;
  return count % 2 === 1;
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

async function fetchToolPreview(url: URL): Promise<string> {
  const r = await fetch(url);
  const text = await r.text();
  let body: any;
  try {
    body = JSON.parse(text);
  } catch {
    throw new Error("工具数据接口返回非 JSON 响应");
  }
  if (!r.ok) throw new Error(body?.error?.message || "工具数据接口请求失败");
  return String(body.output_preview ?? "");
}

function firstHeading(md: string): string {
  return md.match(/^##?\s+(.+)$/m)?.[1]?.trim() ?? "";
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
    const heading = lines[i].match(/^##\s+(.+)$/);
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

function CorpusSearchCard(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  const args = parseArgs(props.tool);
  const query = String(args.query ?? "");
  const preview = props.tool.output_preview ?? "";
  const parsed = safeParseJson(preview) as
    | { hits?: CorpusHit[]; total_corpus_docs?: number } | null;
  // Fall back to brace-balanced extraction when the preview was truncated
  // mid-record (preview cap < full output).
  const hits: CorpusHit[] = parsed?.hits
    ?? (extractObjectsAfter(preview, "\"hits\"") as CorpusHit[]);
  return (
    <CardShell
      status={props.tool.status}
      badge="corpus"
      detail={query ? `"${query}"` : undefined}
      callId={props.tool.call_id}
      size="normal"
    >
      <Show
        when={hits.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
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
        "font-size": "12px",
        color: "var(--ink-0)",
        flex: 1,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>{props.hit.title}</span>
      <span style={{
        "font-family": "var(--font-mono)",
        "font-size": "9.5px",
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
}

/** Pull page title (first `# heading`), short description, and thumbnail
 *  (first markdown image) out of the truncated readability output. */
export function extractLinkMeta(md: string): LinkMeta {
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

  const meta = createMemo(() => extractLinkMeta(props.tool.output_preview ?? ""));

  return (
    <div
      onClick={url ? () => window.open(url, "_blank", "noopener,noreferrer") : undefined}
      data-call-id={props.tool.call_id}
      class="lk-artifact-card is-normal"
      style={{
        background: "var(--bg-1)",
        border: "1px solid var(--bg-2)",
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
          "font-size": "10.5px",
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
        </div>
        <Show
          when={meta().title || meta().description}
          fallback={
            <div style={{ color: "var(--ink-3)", "font-size": "12px" }}>
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
              "font-size": "12px",
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

/* ---------- use_skill ---------- */

function UseSkillCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const skill = String(args.name ?? args.skill ?? "").replace(/[-_]/g, " ");
  const body = () => props.tool.output_preview ?? "";
  const headings = createMemo(() =>
    body()
      .split("\n")
      .map((line) => line.match(/^#{2,3}\s+(.+)$/)?.[1]?.trim())
      .filter(Boolean)
      .slice(0, 6) as string[]
  );

  return (
    <CardShell
      status={props.tool.status}
      badge="skill"
      detail={skill || undefined}
      callId={props.tool.call_id}
      size="compact"
    >
      <Show
        when={headings().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="Skill loaded.">
              Loading skill…
            </Show>
          </div>
        }
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
          <div style={{
            "font-size": "13px",
            color: "var(--ink-0)",
            "line-height": 1.35,
            "font-weight": 600,
          }}>
            {skill ? skill.replace(/\b\w/g, (m) => m.toUpperCase()) : "Research Skill"}
          </div>
          <div style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "6px",
          }}>
            <For each={headings()}>{(h) => (
              <span style={{
                "font-family": "var(--font-mono)",
                "font-size": "10.5px",
                color: "var(--ink-2)",
                padding: "4px 7px",
                background: "rgba(217,119,87,0.08)",
                border: "1px solid rgba(217,119,87,0.18)",
                "border-radius": "999px",
                "white-space": "nowrap",
              }}>{h}</span>
            )}</For>
          </div>
        </div>
      </Show>
    </CardShell>
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
  const info = createMemo(() => parseCompanyInfo(props.tool.output_preview ?? ""));
  const selectedBasics = () => info().basics.filter(([k]) =>
    ["公司名", "所在地", "成立日期", "员工数", "主营业务"].includes(k)
  );
  const selectedMetrics = () => info().metrics.filter(([k]) =>
    ["ROE", "毛利率", "资产负债率", "PE(TTM)", "PB", "营收同比", "净利润同比"].includes(k)
  );

  return (
    <>
      <CardShell
        status={props.tool.status}
        badge="company"
        detail={info().title || tsCode}
        callId={props.tool.call_id}
        size="wide"
        onClick={() => setOpen(true)}
      >
        <Show
          when={info().title || selectedBasics().length || selectedMetrics().length}
          fallback={
            <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
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
              <div style={{ display: "grid", "grid-template-columns": "repeat(2, minmax(0, 1fr))", gap: "6px" }}>
                <For each={selectedBasics().slice(0, 4)}>{([k, v]) => (
                  <div style={{
                    padding: "6px 8px",
                    background: "rgba(255,255,255,0.025)",
                    border: "1px solid var(--line-1)",
                    "border-radius": "6px",
                    "min-width": 0,
                  }}>
                    <div style={{ "font-family": "var(--font-mono)", "font-size": "9.5px", color: "var(--ink-3)" }}>{k}</div>
                    <div style={{ "font-size": "11.5px", color: "var(--ink-1)", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>{v}</div>
                  </div>
                )}</For>
              </div>
              <Show when={info().intro}>
                <div style={{
                  "font-size": "12px",
                  color: "var(--ink-2)",
                  "line-height": 1.55,
                  display: "-webkit-box",
                  "-webkit-line-clamp": "4",
                  "-webkit-box-orient": "vertical",
                  overflow: "hidden",
                }}>{info().intro}</div>
              </Show>
              <div style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--clay-soft)" }}>
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
                    <div style={{ "font-family": "var(--font-mono)", "font-size": "9.5px", color: "var(--ink-3)" }}>{k}</div>
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
                  <div style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)", "margin-bottom": "3px" }}>{k}</div>
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
                      <div style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)", "margin-bottom": "4px" }}>{k}</div>
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
  ["overview", "Overview"],
  ["statements", "Statements"],
  ["statistics", "Statistics"],
  ["dividends", "Dividends"],
  ["earnings", "Earnings"],
  ["revenue", "Revenue"],
];

const STATEMENT_TABS: Array<[StatementTab, string]> = [
  ["income", "Income statement"],
  ["balance", "Balance sheet"],
  ["cashflow", "Cash flow"],
];

function FinancialsCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const ticker = String(args.ticker ?? "");
  const market = String(args.market ?? "");
  const [open, setOpen] = createSignal(false);
  const [periods, setPeriods] = createSignal(12);
  const [periodMode, setPeriodMode] = createSignal<PeriodMode>("quarterly");
  const [tab, setTab] = createSignal<FinancialTab>("overview");
  const [statementTab, setStatementTab] = createSignal<StatementTab>("income");
  const [preview, setPreview] = createSignal(props.tool.output_preview ?? "");
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal("");
  const tables = createMemo(() => parseMarkdownTables(preview()));
  const incomeTable = () => findFinancialTable(tables(), ["利润表", "Income"]);
  const balanceTable = () => findFinancialTable(tables(), ["资产负债表", "Balance"]);
  const cashflowTable = () => findFinancialTable(tables(), ["现金流", "Cash Flow"]);
  const ratioTable = () => findFinancialTable(tables(), ["财务指标", "Ratios"]);
  const facts = createMemo(() => financialFacts(tables()));
  const activeStatementTable = () =>
    statementTab() === "income" ? incomeTable()
    : statementTab() === "balance" ? balanceTable()
    : cashflowTable();

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
    if (props.tool.status === "completed" && ticker && market) void load(periods());
  });

  return (
    <>
      <CardShell
        status={props.tool.status}
        badge="financials"
        detail={ticker}
        callId={props.tool.call_id}
        size="wide"
        onClick={() => setOpen(true)}
      >
        <div style={{ display: "flex", "align-items": "center", gap: "8px", "margin-bottom": "10px" }}>
          <PeriodButtons value={periods()} onPick={load} />
          <Show when={loading()}>
            <span style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)" }}>
              loading statements…
            </span>
          </Show>
          <Show when={error()}>
            <span style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "#d97070", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
              {error()}
            </span>
          </Show>
        </div>
        <Show
          when={tables().length > 0}
          fallback={
            <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
              <Show when={props.tool.status === "in_progress"} fallback="No financial tables.">
                Fetching financials…
              </Show>
            </div>
          }
        >
          <div style={{
            display: "grid",
            "grid-template-columns": "minmax(220px, 0.75fr) minmax(0, 1.25fr)",
            gap: "12px",
          }}>
            <FinancialFactGrid facts={facts()} />
            <div style={{ display: "grid", "grid-template-columns": "repeat(2, minmax(0, 1fr))", gap: "10px" }}>
              <FinancialMiniChart title="Revenue" table={incomeTable()} columnHints={["营收", "Revenue"]} color="#3f82f6" />
              <FinancialMiniChart title="Profitability" table={ratioTable()} columnHints={["ROE"]} color="#d9a76c" />
              <FinancialMiniChart title="Operating CF" table={cashflowTable()} columnHints={["经营活动", "Operating CF"]} color="#35b7b5" />
              <FinancialMiniChart title="Debt" table={ratioTable()} columnHints={["资产负债率", "Debt"]} color="#e05288" />
            </div>
          </div>
          <div style={{
            "margin-top": "10px",
            "font-family": "var(--font-mono)",
            "font-size": "10px",
            color: "var(--clay-soft)",
          }}>
            点击查看完整财务面板
          </div>
        </Show>
      </CardShell>
      <Show when={open()}>
        <ArtifactModal
          title={ticker}
          subtitle={`Financials · ${periods()} periods`}
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
                  <span class="lk-fin-note">loading…</span>
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
              <FinancialDividendsView table={ratioTable()} mode={periodMode()} periods={periods()} />
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
  return (
    <div style={{
      display: "inline-flex",
      gap: "4px",
      padding: "2px",
      background: "rgba(255,255,255,0.03)",
      border: "1px solid var(--line-1)",
      "border-radius": "6px",
    }}>
      <For each={[8, 12, 24]}>{(n) => (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); props.onPick(n); }}
          style={{
            appearance: "none",
            border: "0",
            background: props.value === n ? "rgba(217,119,87,0.18)" : "transparent",
            color: props.value === n ? "var(--clay-soft)" : "var(--ink-3)",
            "font-family": "var(--font-mono)",
            "font-size": "10.5px",
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
      <button type="button" class={props.value === "annual" ? "active" : ""} onClick={() => props.onPick("annual")}>Annual</button>
      <button type="button" class={props.value === "quarterly" ? "active" : ""} onClick={() => props.onPick("quarterly")}>Quarterly</button>
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
          title="Growth and Profitability"
          table={props.income}
          mode={props.mode}
          series={[
            { label: "Revenue", hints: ["营收", "Revenue"], color: "#3f82f6" },
            { label: "Net income", hints: ["归母净利", "Net Income"], color: "#35c4d3" },
          ]}
        />
        <FinancialSeriesChart
          title="Revenue to Profit Conversion"
          table={props.income}
          mode={props.mode}
          series={[
            { label: "Revenue", hints: ["营收", "Revenue"], color: "#35c4d3" },
            { label: "Operating income", hints: ["营业利润", "Operating Income"], color: "#3f82f6" },
            { label: "Net income", hints: ["归母净利", "Net Income"], color: "#ff4f8b" },
          ]}
        />
        <FinancialSeriesChart
          title="Financial Health"
          table={props.balance}
          mode={props.mode}
          series={[
            { label: "Assets", hints: ["总资产", "Total Assets"], color: "#3f82f6" },
            { label: "Liabilities", hints: ["总负债", "Total Liabilities"], color: "#35c4d3" },
            { label: "Debt", hints: ["总债务", "Total Debt"], color: "#ff4f8b" },
          ]}
        />
        <FinancialSeriesChart
          title="Quality Ratios"
          table={props.ratios}
          mode={props.mode}
          series={[
            { label: "ROE", hints: ["ROE"], color: "#ff9d00" },
            { label: "Gross margin", hints: ["毛利率", "Gross Margin"], color: "#43d7c4" },
            { label: "Debt ratio", hints: ["资产负债率", "Debt"], color: "#ff4f8b" },
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
  const chartSeries = () => {
    if (props.statement === "balance") {
      return [
        { label: "Total assets", hints: ["总资产", "Total Assets"], color: "#3f82f6" },
        { label: "Total liabilities", hints: ["总负债", "Total Liabilities"], color: "#35c4d3" },
      ];
    }
    if (props.statement === "cashflow") {
      return [
        { label: "Operating CF", hints: ["经营活动", "Operating CF"], color: "#35c4d3" },
        { label: "Investing CF", hints: ["投资活动", "Investing CF"], color: "#ff9d00" },
        { label: "Financing CF", hints: ["筹资活动", "Financing CF"], color: "#ff4f8b" },
      ];
    }
    return [
      { label: "Revenue", hints: ["营收", "Revenue"], color: "#3f82f6" },
      { label: "Operating income", hints: ["营业利润", "Operating Income"], color: "#35c4d3" },
      { label: "Net income", hints: ["归母净利", "Net Income"], color: "#ff9d00" },
    ];
  };
  return (
    <div class="lk-fin-tab-body">
      <FinancialSeriesChart title="Statement trend" table={props.table} mode={props.mode} series={chartSeries()} height={210} />
      <FinancialMatrixTable table={props.table} mode={props.mode} periods={props.periods} />
    </div>
  );
}

function FinancialStatisticsView(props: { table?: MarkdownTable; mode: PeriodMode; periods: number }) {
  return (
    <div class="lk-fin-tab-body">
      <FinancialSeriesChart
        title="Profitability Ratios"
        table={props.table}
        mode={props.mode}
        series={[
          { label: "ROE", hints: ["ROE"], color: "#3f82f6" },
          { label: "Gross margin", hints: ["毛利率", "Gross Margin"], color: "#35c4d3" },
          { label: "Net margin", hints: ["净利率", "Net Margin"], color: "#ff9d00" },
        ]}
        height={210}
      />
      <FinancialMatrixTable table={props.table} mode={props.mode} periods={props.periods} grouped />
    </div>
  );
}

function FinancialDividendsView(props: { table?: MarkdownTable; mode: PeriodMode; periods: number }) {
  const dividendHints = ["每股股利", "Dividend"];
  return (
    <div class="lk-fin-tab-body">
      <FinancialSeriesChart
        title="Dividend History"
        table={props.table}
        mode={props.mode}
        series={[
          { label: "Dividend per share", hints: dividendHints, color: "#3f82f6" },
          { label: "Payout ratio", hints: ["派息率", "Payout"], color: "#35c4d3" },
        ]}
        height={210}
      />
      <FinancialMatrixTable table={props.table} mode={props.mode} periods={props.periods} rowHints={[...dividendHints, "Payout"]} />
    </div>
  );
}

function FinancialEarningsView(props: {
  income?: MarkdownTable;
  ratios?: MarkdownTable;
  mode: PeriodMode;
  periods: number;
}) {
  return (
    <div class="lk-fin-tab-body">
      <FinancialSeriesChart
        title="EPS"
        table={props.ratios}
        mode={props.mode}
        series={[{ label: "Reported EPS", hints: ["EPS"], color: "#3f82f6" }]}
        height={210}
      />
      <FinancialSeriesChart
        title="Earnings"
        table={props.income}
        mode={props.mode}
        series={[
          { label: "Revenue", hints: ["营收", "Revenue"], color: "#ff9d00" },
          { label: "Net income", hints: ["归母净利", "Net Income"], color: "#3f82f6" },
        ]}
        height={210}
      />
      <FinancialMatrixTable table={props.ratios} mode={props.mode} periods={props.periods} rowHints={["EPS", "ROE", "每股", "Per Share"]} />
    </div>
  );
}

function FinancialRevenueView(props: { table?: MarkdownTable; mode: PeriodMode; periods: number }) {
  return (
    <div class="lk-fin-tab-body">
      <FinancialSeriesChart
        title="Revenue"
        table={props.table}
        mode={props.mode}
        series={[
          { label: "Revenue", hints: ["营收", "Revenue"], color: "#3f82f6" },
          { label: "Gross profit", hints: ["毛利", "Gross Profit"], color: "#35c4d3" },
          { label: "Net income", hints: ["归母净利", "Net Income"], color: "#ff9d00" },
        ]}
        height={230}
      />
      <FinancialMatrixTable table={props.table} mode={props.mode} periods={props.periods} rowHints={["营收", "Revenue", "毛利", "Gross", "营业收入"]} />
    </div>
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
  add("营收", financialCell(income, ["营收", "Revenue"]));
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
      <div style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)", "margin-bottom": "4px" }}>
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

function tableForMode(table: MarkdownTable | undefined, mode: PeriodMode): MarkdownTable | undefined {
  if (!table) return undefined;
  const filtered = table.rows.filter((row) =>
    mode === "annual" ? isAnnualPeriod(row[0] ?? "") : !isAnnualPeriod(row[0] ?? "")
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
}) {
  const plot = () => {
    const table = tableForMode(props.table, props.mode);
    if (!table) return { rows: [], series: [] as Array<FinancialSeriesSpec & { idx: number }> };
    const series = props.series
      .map((s) => ({ ...s, idx: columnIndex(table, s.hints) }))
      .filter((s) => s.idx >= 0);
    const rows = table.rows
      .slice(0, 8)
      .map((row) => ({
        label: row[0] ?? "",
        values: series.map((s) => financialNumber(row[s.idx] ?? "")),
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
  return (
    <section class="lk-fin-chart-card">
      <div class="lk-fin-section-head">
        <span>{props.title}</span>
        <em>{props.mode === "annual" ? "Annual" : "Quarterly"}</em>
      </div>
      <Show
        when={plot().rows.length > 0 && plot().series.length > 0}
        fallback={<FinancialEmpty label="No series available" />}
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
                      x={x0 + sIndex() * barW}
                      y={y}
                      width={barW * 0.74}
                      height={barH}
                      rx="2"
                      fill={s.color}
                      opacity="0.88"
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
}) {
  const view = () => {
    const table = tableForMode(props.table, props.mode);
    if (!table) return { periods: [] as string[], metrics: [] as Array<{ label: string; values: string[]; color: string }> };
    const rows = table.rows.slice(0, props.periods).reverse();
    const metricHeaders = table.headers.slice(1);
    const filtered = props.rowHints?.length
      ? metricHeaders.filter((h) => props.rowHints!.some((hint) => h.toLowerCase().includes(hint.toLowerCase())))
      : metricHeaders;
    const colors = ["#3f82f6", "#35c4d3", "#ff9d00", "#ff4f8b", "#9b7cff", "#43d7c4"];
    return {
      periods: rows.map((row) => row[0] ?? ""),
      metrics: filtered.slice(0, props.grouped ? 28 : 18).map((header, i) => {
        const idx = table.headers.indexOf(header);
        return {
          label: header,
          values: rows.map((row) => row[idx] ?? "—"),
          color: colors[i % colors.length],
        };
      }),
    };
  };
  return (
    <section class="lk-fin-table-card">
      <Show when={view().metrics.length > 0} fallback={<FinancialEmpty label="No table data available" />}>
        <div class="lk-fin-matrix">
          <table>
            <thead>
              <tr>
                <th>Currency: CNY</th>
                <For each={view().periods}>{(period) => <th>{period}</th>}</For>
              </tr>
            </thead>
            <tbody>
              <For each={view().metrics}>{(metric) => (
                <tr>
                  <td><i style={{ background: metric.color }} />{metric.label}</td>
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

function FinancialEmpty(props: { label: string }) {
  return <div class="lk-fin-empty">{props.label}</div>;
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
          <div style={{ "margin-left": "auto", "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)" }}>
            {rows()[rows().length - 1]?.label}
          </div>
        </Show>
      </div>
      <Show
        when={rows().length > 0}
        fallback={<div style={{ height: props.large ? "120px" : "68px", color: "var(--ink-3)", "font-size": "11px" }}>No series</div>}
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
            const color = row.value >= 0 ? props.color : "#6fb98a";
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
        "font-size": "12px",
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
        "font-size": "10px",
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

/* ---------- get_capital_flow ---------- */

function numericCell(raw: string): number {
  const n = parseFloat(raw.replace(/[,，%]/g, ""));
  return Number.isFinite(n) ? n : 0;
}

function CapitalFlowCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tsCode = String(args.ts_code ?? "");
  const tables = createMemo(() => parseMarkdownTables(props.tool.output_preview ?? ""));
  const stockTable = () => tables().find((t) => t.headers.some((h) => h.includes("净流入额")));
  const northTable = () => tables().find((t) => t.headers.some((h) => h.includes("北向")));
  const recentStock = () => stockTable()?.rows.slice(-7) ?? [];
  const recentNorth = () => northTable()?.rows.slice(-5) ?? [];
  const hasStockRows = () => recentStock().length > 0;
  const northStopped = () => recentNorth().some((row) => row[0] >= "2024-08-20");
  const hasNorthRows = () => recentNorth().length > 0 && !northStopped();
  const northUnavailable = () => capitalFlowNorthboundUnavailable(props.tool.output_preview ?? "") || northStopped();
  const paneCount = () => [hasStockRows(), hasNorthRows(), northUnavailable()].filter(Boolean).length;
  const stockNetSum = () => recentStock().reduce((sum, row) => sum + numericCell(row[row.length - 1] ?? "0"), 0);
  const northNetSum = () => recentNorth().reduce((sum, row) => sum + numericCell(row[1] ?? "0"), 0);

  return (
    <CardShell
      status={props.tool.status}
      badge="capital · flow"
      detail={tsCode || "northbound"}
      callId={props.tool.call_id}
      size="wide"
    >
      <Show
        when={hasStockRows() || hasNorthRows() || northUnavailable()}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
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
            <FlowPane title={stockTable()?.title || `${tsCode} 资金流向`} unit="万元" rows={recentStock()} net={stockNetSum()} netIndex={5} />
          </Show>
          <Show when={hasNorthRows()}>
            <FlowPane title={northTable()?.title || "北向资金"} unit="亿" rows={recentNorth()} net={northNetSum()} netIndex={1} />
          </Show>
          <Show when={northUnavailable() && !hasNorthRows()}>
            <FlowNoticePane />
          </Show>
        </div>
      </Show>
    </CardShell>
  );
}

function capitalFlowNorthboundUnavailable(md: string): boolean {
  return md.includes("northbound_daily_unavailable") || md.includes("日度净流入已停更");
}

function FlowNoticePane() {
  return (
    <div style={{
      padding: "10px 11px",
      background: "rgba(217,119,87,0.045)",
      border: "1px solid rgba(217,119,87,0.18)",
      "border-radius": "7px",
      color: "var(--ink-2)",
      "font-size": "11.5px",
      "line-height": 1.55,
    }}>
      <div style={{ color: "var(--ink-0)", "font-weight": 700, "margin-bottom": "4px" }}>
        北向资金日度净流入已停更
      </div>
      <div>
        交易所自 2024-08-20 起停止日度北向披露；这里不再展示空白或零值面板。后续应接入季度持股或沪深港通成交类替代数据。
      </div>
    </div>
  );
}

function FlowPane(props: {
  title: string;
  unit: string;
  rows: string[][];
  net: number;
  netIndex: number;
}) {
  const maxAbs = () => Math.max(1, ...props.rows.map((row) => Math.abs(numericCell(row[props.netIndex] ?? "0"))));
  const netColor = () => props.net >= 0 ? "#d97757" : "#6fb98a";
  return (
    <div style={{
      padding: "9px 10px",
      background: "rgba(255,255,255,0.02)",
      border: "1px solid var(--line-1)",
      "border-radius": "7px",
      "min-width": 0,
    }}>
      <div style={{ display: "flex", "align-items": "baseline", gap: "8px", "margin-bottom": "8px" }}>
        <div style={{ "font-size": "12.5px", color: "var(--ink-0)", "font-weight": 650, overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
          {props.title}
        </div>
        <div style={{ "margin-left": "auto", "font-family": "var(--font-mono)", "font-size": "11px", color: netColor(), "font-weight": 700 }}>
          {props.net >= 0 ? "+" : ""}{props.net.toFixed(1)}
        </div>
      </div>
      <div style={{ display: "flex", "flex-direction": "column", gap: "5px" }}>
        <For each={props.rows}>{(row) => {
          const net = numericCell(row[props.netIndex] ?? "0");
          const width = Math.max(5, Math.min(100, Math.abs(net) / maxAbs() * 100));
          const color = net >= 0 ? "#d97757" : "#6fb98a";
          return (
            <div style={{ display: "grid", "grid-template-columns": "72px minmax(0, 1fr) 74px", gap: "7px", "align-items": "center" }}>
              <span style={{ "font-family": "var(--font-mono)", "font-size": "10px", color: "var(--ink-3)" }}>{row[0]}</span>
              <span style={{ height: "6px", background: "rgba(255,255,255,0.035)", "border-radius": "99px", overflow: "hidden" }}>
                <span style={{ display: "block", height: "100%", width: `${width}%`, background: color, opacity: 0.72, "margin-left": net >= 0 ? "0" : "auto" }} />
              </span>
              <span style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color, "text-align": "right" }}>
                {net >= 0 ? "+" : ""}{net.toFixed(1)} {props.unit}
              </span>
            </div>
          );
        }}</For>
      </div>
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
  const rows = createMemo(() => parseTradingViewTable(props.tool.output_preview ?? ""));
  const multi = () => rows().length > 1;
  const detail = () => multi() ? `${rows().length} symbols` : tickers.join(", ");

  return (
    <CardShell
      status={props.tool.status}
      badge="market · quote"
      detail={detail()}
      callId={props.tool.call_id}
      size={multi() ? "wide" : "normal"}
    >
      <Show
        when={rows().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
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
  const color = up == null ? "var(--ink-3)" : up ? "#d97757" : "#6fb98a";
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
  const color = up == null ? "var(--ink-3)" : up ? "#d97757" : "#6fb98a";
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
            "font-size": "12px",
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
      <div style={{ "font-family": "var(--font-mono)", "font-size": "9.5px", color: "var(--ink-3)", "margin-bottom": "3px" }}>
        {props.label}
      </div>
      <div style={{
        "font-family": "var(--font-mono)",
        "font-size": "11.5px",
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

function parseTushareTable(md: string): OhlcvRow[] {
  const rows: OhlcvRow[] = [];
  const re = /^\|\s*(\d{4}-\d{2}-\d{2}(?:\s+\d{2}:\d{2}(?::\d{2})?)?)\s*\|\s*([\d.]+)\s*\|\s*([\d.]+)\s*\|\s*([\d.]+)\s*\|\s*([\d.]+)\s*\|\s*([+\-\d.]+)\s*\|\s*([\d.]+)\s*\|/;
  for (const line of md.split("\n")) {
    const m = line.match(re);
    if (!m) continue;
    const [, date, o, h, l, c, p, v] = m;
    rows.push({
      date,
      open: parseFloat(o),
      high: parseFloat(h),
      low: parseFloat(l),
      close: parseFloat(c),
      pctChg: parseFloat(p),
      vol: parseFloat(v),
    });
  }
  return rows;
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
    series = chart.addSeries(CandlestickSeries, {
      // Red-up / green-down — Chinese A-share convention since this card
      // is primarily for tushare CN equities.
      upColor: "#d97757",
      downColor: "#6fb98a",
      borderVisible: false,
      wickUpColor: "#d97757",
      wickDownColor: "#6fb98a",
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

function TushareCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tsCode = String(args.ts_code ?? "");
  const rows = createMemo(() => parseTushareTable(props.tool.output_preview ?? ""));
  const last = () => rows().length > 0 ? rows()[rows().length - 1] : null;
  const detail = () => {
    const r = last();
    if (!r) return tsCode;
    const sign = r.pctChg >= 0 ? "+" : "";
    return `${tsCode} · ${r.close.toFixed(2)} (${sign}${r.pctChg.toFixed(2)}%)`;
  };

  return (
    <CardShell
      status={props.tool.status}
      badge="market · A 股"
      detail={detail()}
      callId={props.tool.call_id}
      size="wide"
    >
      <Show
        when={rows().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No data.">
              Fetching A-share OHLCV…
            </Show>
          </div>
        }
      >
        <Candlestick data={rows()} height={180} />
      </Show>
    </CardShell>
  );
}

function CandlestickToolCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const ticker = String(args.ticker ?? "");
  const market = String(args.market ?? "");
  const isAshare = market === "a_share" || market === "ashare" || market === "cn";
  const initialInterval = String(args.interval ?? "1d");
  const [interval, setInterval] = createSignal(isAshare && initialInterval === "15m" ? "1d" : initialInterval);
  const [preview, setPreview] = createSignal(props.tool.output_preview ?? "");
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal("");
  const rows = createMemo(() => parseCandlestickTable(preview()));
  const last = () => rows().length > 0 ? rows()[rows().length - 1] : null;
  const detail = () => {
    const r = last();
    if (!r) return [ticker, market].filter(Boolean).join(" · ");
    const sign = r.pctChg >= 0 ? "+" : "";
    return `${ticker} · ${r.close.toFixed(2)} (${sign}${r.pctChg.toFixed(2)}%)`;
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
    if (props.tool.status === "completed" && ticker && market) void load(interval());
  });

  return (
    <CardShell
      status={props.tool.status}
      badge="market · k-line"
      detail={detail()}
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
                  "font-size": "10.5px",
                  padding: "4px 8px",
                  "border-radius": "4px",
                  cursor: disabled ? "not-allowed" : "pointer",
                }}
              >{label}</button>
            );
          }}</For>
        </div>
        <Show when={isAshare}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)" }}>
            15m 等 pytdx 接入
          </span>
        </Show>
        <Show when={loading()}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "var(--ink-3)" }}>
            loading candles…
          </span>
        </Show>
        <Show when={error()}>
          <span style={{ "font-family": "var(--font-mono)", "font-size": "10.5px", color: "#d97070", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
            {error()}
          </span>
        </Show>
      </div>
      <Show
        when={rows().length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No candles.">
              Fetching candlesticks…
            </Show>
          </div>
        }
      >
        <Candlestick data={rows()} height={260} />
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
  const preview = props.tool.output_preview ?? "";
  const parsed = safeParseJson(preview) as
    | { filings?: SecFiling[]; company_name?: string } | null;
  const filings: SecFiling[] = parsed?.filings
    ?? (extractObjectsAfter(preview, "\"filings\"") as SecFiling[]);
  return (
    <CardShell
      status={props.tool.status}
      badge="sec · edgar"
      detail={parsed?.company_name ? `${ticker} · ${parsed.company_name}` : ticker}
      callId={props.tool.call_id}
      size="normal"
    >
      <Show
        when={filings.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
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
    <CardShell status={props.tool.status} badge={props.tool.name} detail={summary} callId={props.tool.call_id} size="compact">
      <Show when={props.tool.output_preview}>
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "11px",
          color: "var(--ink-2)",
          "white-space": "pre-wrap",
          "max-height": "120px",
          overflow: "hidden",
        }}>{props.tool.output_preview}</div>
      </Show>
    </CardShell>
  );
}
