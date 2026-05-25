// Live mode — talks to the real Rust gateway via SSE.
// Uses the same chat primitives as the fixture scenes (UserMsg / AgentMsg /
// StreamText / Composer); only the data source is different.

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { AgentMsg, Composer, UserMsg, type SlashCommand } from "./Chat";
import { EventsPanel } from "./EventsPanel";
import { InsightSidebar, type AgentPlanView } from "./BrainWidget";
import { Rail, TopBar } from "./Workbench";
import { NavRail } from "../App";
import { SafeMarkdown } from "./SafeMarkdown";
import { ArtifactPanel, extractLinkMeta, toolDisplayName, type ArtifactEventView, type DecisionDraftView, type ToolUiArtifact } from "./ArtifactCards";
import { SessionMenu, type SessionRow } from "./SessionMenu";
import { CorpusDocModal, type CorpusDoc } from "./CorpusDocModal";
import type { Scene } from "../scenes";

const DEFAULT_SESSION_ID = "live";

function readSessionFromLocation(): string {
  const match = window.location.pathname.match(/^\/sessions\/([^/?#]+)/);
  if (match?.[1]) return decodeURIComponent(match[1]);
  const h = window.location.hash.replace(/^#/, "");
  if (h.startsWith("s/")) return h.slice(2);
  return DEFAULT_SESSION_ID;
}
function writeSessionToLocation(id: string) {
  if (id === DEFAULT_SESSION_ID) {
    if (window.location.pathname !== "/" || window.location.hash) {
      history.pushState(null, "", "/");
    }
  } else {
    const path = `/sessions/${encodeURIComponent(id)}`;
    if (window.location.pathname !== path || window.location.hash) {
      history.pushState(null, "", path);
    }
  }
}

interface SearchCall {
  status: "in_progress" | "completed" | string;
  action: "search" | "open_page" | "find_in_page" | "other" | "unknown" | string;
  detail: string;
  queries?: string[];
  sources?: string[];
}

/** Stable id for a search call so chat-row clicks can locate the canvas tile.
 *  Tool calls have a real `call_id`; searches don't, so we hash action+detail. */
function searchKey(s: SearchCall): string {
  const detail = s.detail || s.queries?.[0] || "";
  return `search:${s.action}:${detail.slice(0, 120)}`;
}

/** Scroll the canvas card with the matching `data-call-id` into view and
 *  briefly highlight it. */
function scrollToCanvasCard(callId: string | undefined) {
  if (!callId) return;
  const el = document.querySelector(`[data-call-id="${(window as any).CSS ? CSS.escape(callId) : callId}"]`) as HTMLElement | null;
  if (!el) return;
  el.scrollIntoView({ behavior: "smooth", block: "center" });
  el.classList.remove("lk-card-flash"); // restart animation if already added
  void el.offsetWidth;
  el.classList.add("lk-card-flash");
  window.setTimeout(() => el.classList.remove("lk-card-flash"), 900);
}

interface ToolCall {
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

interface NarrationStep {
  turn: number;
  text: string;
}

interface ArtifactEvent extends ArtifactEventView {
  id: string;
  kind: "narration" | "narration_group" | "search" | "tool" | "decision";
  narration?: NarrationStep;
  narrations?: NarrationStep[];
  search?: SearchCall;
  tool?: ToolCall;
  decision?: DecisionDraftPayload;
}

interface ClarificationOption {
  label: string;
  description: string;
}

interface ClarificationQuestion {
  id: string;
  header: string;
  question: string;
  options: ClarificationOption[];
}

interface ClarificationPayload {
  question: string;
  questions: ClarificationQuestion[];
}

interface DecisionDraftPayload extends DecisionDraftView {}

interface LiveMsg {
  /** `compaction_summary` rows are written by /compact and rendered as a
   *  collapsible system card inside the same session. */
  role: "user" | "agent" | "compaction_summary" | "decision_draft";
  text: string;
  ts: string;
  streaming?: boolean;
  searches?: SearchCall[];
  tool_calls?: ToolCall[];
  narrations?: NarrationStep[];
  artifacts?: ArtifactEvent[];
  clarification?: ClarificationPayload;
  /** Final elapsed seconds for the agent reply, frozen at message_end. */
  total_sec?: number;
  decision_draft?: DecisionDraftPayload;
  msg_seq?: number;
  raw_ts?: string;
  hidden?: boolean;
}

/** Context-compacted divider — shown after compaction in the same session.
 *  Prominent horizontal rule signals the break; summary expands on click. */
function CompactionSummaryCard(props: { time: string; markdown: string }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="lk-msg lk-msg-compaction" data-who="system" style={{ margin: "16px 0 8px" }}>
      <div
        onClick={() => setOpen((v) => !v)}
        style={{
          display: "flex",
          "align-items": "center",
          gap: "10px",
          cursor: "pointer",
          "user-select": "none",
        }}
      >
        <div style={{
          flex: 1,
          height: "1px",
          background: "linear-gradient(90deg, transparent, rgba(217,119,87,0.5))",
        }} />
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "10.5px",
          color: "rgba(217,119,87,0.8)",
          "letter-spacing": "0.12em",
          "text-transform": "uppercase",
          "white-space": "nowrap",
          padding: "2px 8px",
          border: "1px solid rgba(217,119,87,0.3)",
          "border-radius": "20px",
          background: "rgba(217,119,87,0.06)",
        }}>
          context compacted · {open() ? "收起 ▴" : "展开 ▾"}
        </span>
        <div style={{
          flex: 1,
          height: "1px",
          background: "linear-gradient(90deg, rgba(217,119,87,0.5), transparent)",
        }} />
      </div>
      <Show when={open()}>
        <div style={{
          margin: "10px 0 4px",
          padding: "12px 14px",
          background: "rgba(217,119,87,0.04)",
          border: "1px dashed rgba(217,119,87,0.25)",
          "border-radius": "6px",
          "font-size": "13px",
          "line-height": "1.6",
        }}>
          <div style={{
            "font-family": "var(--font-mono)",
            "font-size": "10px",
            color: "var(--ink-3)",
            "margin-bottom": "8px",
          }}>{props.time}</div>
          <SafeMarkdown source={props.markdown} />
        </div>
      </Show>
    </div>
  );
}

/** Inline divider shown at the bottom of messages while compaction is running. */
function CompactingDivider() {
  return (
    <div style={{ margin: "16px 0 8px", display: "flex", "align-items": "center", gap: "10px" }}>
      <div style={{
        flex: 1,
        height: "1px",
        background: "linear-gradient(90deg, transparent, rgba(217,119,87,0.4))",
      }} />
      <span style={{
        "font-family": "var(--font-mono)",
        "font-size": "10.5px",
        color: "rgba(217,119,87,0.7)",
        "letter-spacing": "0.1em",
        "text-transform": "uppercase",
        "white-space": "nowrap",
        padding: "2px 8px",
        border: "1px dashed rgba(217,119,87,0.3)",
        "border-radius": "20px",
        animation: "lk-blink 1.6s ease-in-out infinite",
      }}>
        compacting…
      </span>
      <div style={{
        flex: 1,
        height: "1px",
        background: "linear-gradient(90deg, rgba(217,119,87,0.4), transparent)",
      }} />
    </div>
  );
}

function DecisionDraftCard(props: { time: string; draft: DecisionDraftPayload }) {
  const [status, setStatus] = createSignal<"pending" | "confirmed" | "rejected">("pending");
  const [busy, setBusy] = createSignal(false);

  async function act(action: "confirm" | "reject") {
    setBusy(true);
    try {
      const r = await fetch(`/api/v1/deliverables/${props.draft.deliverable_id}/${action}`, { method: "POST" });
      if (r.ok) setStatus(action === "confirm" ? "confirmed" : "rejected");
    } finally {
      setBusy(false);
    }
  }

  const dirColor = () => props.draft.direction === "long" ? "rgba(100,200,120,0.15)" : "rgba(217,112,112,0.12)";
  const dirBorder = () => props.draft.direction === "long" ? "rgba(100,200,120,0.4)" : "rgba(217,112,112,0.4)";

  return (
    <div style={{
      border: `1px solid ${dirBorder()}`,
      "border-radius": "8px",
      padding: "12px 14px",
      margin: "6px 0",
      background: dirColor(),
      "font-family": "var(--font-mono)",
    }}>
      <div style={{ display: "flex", "align-items": "center", gap: "10px", "margin-bottom": "8px" }}>
        <span style={{ "font-size": "12px", "font-weight": 700, color: "var(--ink-0)", "text-transform": "uppercase", "letter-spacing": "0.06em" }}>
          {props.draft.direction === "long" ? "▲ 买入" : props.draft.direction === "short" ? "▼ 卖出" : "◆ 平仓"} · {props.draft.ticker}
        </span>
        <Show when={props.draft.size_pct != null}>
          <span style={{ "font-size": "11px", color: "var(--ink-2)" }}>{props.draft.size_pct}%</span>
        </Show>
        <div style={{ flex: 1 }} />
        <span style={{ "font-size": "10px", color: "var(--ink-3)" }}>{props.time}</span>
      </div>
      <div style={{ "font-size": "12px", color: "var(--ink-1)", "line-height": "1.5", "margin-bottom": "10px" }}>
        {props.draft.rationale}
      </div>
      <div style={{ display: "flex", gap: "6px", "font-size": "11px", color: "var(--ink-3)", "margin-bottom": "10px" }}>
        <Show when={props.draft.stop_loss != null}>
          <span>止损 {props.draft.stop_loss}</span>
        </Show>
        <Show when={props.draft.target != null}>
          <span>· 目标 {props.draft.target}</span>
        </Show>
        <Show when={props.draft.horizon_days != null}>
          <span>· 周期 {props.draft.horizon_days}d</span>
        </Show>
      </div>
      <Show
        when={status() === "pending"}
        fallback={
          <div style={{
            "font-size": "11px",
            color: status() === "confirmed" ? "rgba(100,200,120,0.9)" : "rgba(217,112,112,0.9)",
            "font-weight": 600,
            "text-transform": "uppercase",
            "letter-spacing": "0.06em",
          }}>
            {status() === "confirmed" ? "✓ 已确认" : "✗ 已拒绝"}
          </div>
        }
      >
        <div style={{ display: "flex", gap: "8px" }}>
          <button
            onClick={() => void act("confirm")}
            disabled={busy()}
            style={{
              background: "rgba(100,200,120,0.25)",
              border: "1px solid rgba(100,200,120,0.5)",
              color: "rgba(100,220,120,1)",
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              "font-weight": 600,
              padding: "4px 14px",
              "border-radius": "5px",
              cursor: busy() ? "wait" : "pointer",
              "letter-spacing": "0.04em",
            }}
          >确认执行</button>
          <button
            onClick={() => void act("reject")}
            disabled={busy()}
            style={{
              background: "transparent",
              border: "1px solid var(--line-2)",
              color: "var(--ink-2)",
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              padding: "4px 14px",
              "border-radius": "5px",
              cursor: busy() ? "wait" : "pointer",
            }}
          >拒绝</button>
        </div>
      </Show>
    </div>
  );
}

function summarizeSearch(s: SearchCall): string {
  const verb = s.status === "completed" ? "✓" : "▸";
  switch (s.action) {
    case "search":
      return `${verb} ${s.status === "completed" ? "已搜索" : "正在搜索"}: ${s.detail || "…"}`;
    case "open_page": {
      let host = s.detail;
      try { host = new URL(s.detail).hostname; } catch { /* keep raw */ }
      return `${verb} ${s.status === "completed" ? "已打开" : "正在打开"}: ${host}`;
    }
    case "find_in_page":
      return `${verb} ${s.status === "completed" ? "已页内查找" : "正在页内查找"}: ${s.detail}`;
    default:
      return `${verb} ${s.action}`;
  }
}

function summarizeTool(t: ToolCall): string {
  const verb = t.status === "completed" ? "✓" : t.status === "error" ? "✗" : "▸";
  // Pull the most user-meaningful field out of the arguments — URL host for
  // web_fetch, the query string for corpus_search, the symbol for quotes.
  let detail = "";
  if (t.arguments) {
    try {
      const args = JSON.parse(t.arguments);
      if (typeof args.url === "string") {
        try { detail = new URL(args.url).hostname.replace(/^www\./, ""); }
        catch { detail = args.url; }
      } else if (typeof args.query === "string") {
        detail = `"${args.query}"`;
      } else if (typeof args.ts_code === "string") {
        detail = args.ts_code;
      } else if (Array.isArray(args.tickers)) {
        detail = (args.tickers as string[]).slice(0, 3).join(", ");
      } else if (typeof args.ticker === "string") {
        detail = args.ticker;
      }
    } catch { /* show name only */ }
  }
  const name = toolDisplayName(t.name);
  const head = t.status === "completed"
    ? `${verb} ${name}`
    : t.status === "error"
    ? `${verb} ${name} 失败`
    : `${verb} ${name}`;
  return detail ? `${head}: ${detail}` : head;
}

function toolCallFromPayload(p: any): ToolCall {
  return {
    call_id: String(p.call_id ?? ""),
    status: p.status ?? "in_progress",
    name: p.name ?? "",
    arguments: typeof p.arguments === "string" ? p.arguments : undefined,
    output_preview: typeof p.output_preview === "string" ? p.output_preview : undefined,
    output_bytes: typeof p.output_bytes === "number" ? p.output_bytes : undefined,
    ui_artifact: typeof p.ui_artifact === "object" && p.ui_artifact !== null ? p.ui_artifact : undefined,
    cached: typeof p.cached === "boolean" ? p.cached : typeof p.cached_from === "string" ? true : undefined,
    cached_from: typeof p.cached_from === "string" ? p.cached_from : undefined,
    error_kind: typeof p.error_kind === "string" ? p.error_kind : undefined,
    retryable: typeof p.retryable === "boolean" ? p.retryable : undefined,
    partial_status: typeof p.partial_status === "string" ? p.partial_status : undefined,
  };
}

function mergeToolCall(prev: ToolCall, next: ToolCall): ToolCall {
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
  if (next.status === "in_progress" && prev.status !== "in_progress") {
    merged.status = prev.status;
  }
  return merged;
}

function findMatchingToolIndex(calls: ToolCall[], call: ToolCall): number {
  const sameCall = calls.findIndex((t) => t.call_id === call.call_id);
  if (sameCall >= 0) return sameCall;
  if (call.cached === true && call.cached_from) {
    const cachedFrom = calls.findIndex((t) => t.call_id === call.cached_from);
    if (cachedFrom >= 0) return cachedFrom;
  }
  return -1;
}

function upsertToolCall(calls: ToolCall[] | undefined, call: ToolCall): ToolCall[] {
  const out = [...(calls ?? [])];
  const idx = findMatchingToolIndex(out, call);
  if (idx >= 0) out[idx] = mergeToolCall(out[idx], call);
  else out.push(call);
  return out;
}

function summarizeProcess(m: LiveMsg): string {
  let narrations = m.narrations?.length ?? 0;
  let tools = (m.tool_calls ?? []).filter(isCanvasTool).length;
  let searches = m.searches?.length ?? 0;
  if (m.artifacts && m.artifacts.length > 0) {
    narrations = 0;
    searches = 0;
    const toolIds = new Set<string>();
    const searchIds = new Set<string>();
    for (const event of m.artifacts) {
      if (event.kind === "narration" && event.narration) narrations += 1;
      if (event.kind === "narration_group" && event.narrations) narrations += event.narrations.length;
      if (event.kind === "search" && event.search) searchIds.add(event.id);
      if (event.kind === "tool" && event.tool && isCanvasTool(event.tool)) toolIds.add(event.tool.call_id);
    }
    tools = toolIds.size;
    searches = searchIds.size;
  }
  const parts: string[] = [];
  if (narrations > 0) parts.push(`${narrations} 个推理步骤`);
  if (tools > 0) parts.push(`${tools} 个工具`);
  if (searches > 0) parts.push(`${searches} 次网页动作`);
  return parts.length > 0 ? `过程已展开到画布：${parts.join(" · ")}` : "";
}

function firstCanvasTarget(m: LiveMsg): string | undefined {
  const firstArtifact = m.artifacts?.[0]?.id;
  if (firstArtifact) return firstArtifact;
  const firstNarration = m.narrations?.[0];
  if (firstNarration) return `narration:${firstNarration.turn}:0`;
  const firstSearch = m.searches?.[0];
  if (firstSearch) return searchKey(firstSearch);
  return m.tool_calls?.[0]?.call_id;
}

function upsertToolEvent(events: ArtifactEvent[] | undefined, call: ToolCall, turn?: number): ArtifactEvent[] {
  const out = [...(events ?? [])];
  const toolEvents = out
    .map((event, index) => ({ event, index }))
    .filter(({ event }) => event.kind === "tool" && event.tool);
  const idx = toolEvents.find(({ event }) => event.tool && event.tool.call_id === call.call_id)?.index
    ?? (call.cached === true && call.cached_from
      ? toolEvents.find(({ event }) => event.tool?.call_id === call.cached_from)?.index
      : undefined);
  if (idx != null && idx >= 0) {
    const prev = out[idx].tool;
    out[idx] = {
      ...out[idx],
      id: prev?.call_id ?? call.call_id,
      kind: "tool",
      turn: turn ?? out[idx].turn,
      tool: prev ? mergeToolCall(prev, call) : call,
    };
  } else {
    out.push({ id: call.call_id, kind: "tool", turn, tool: call });
  }
  return out;
}

function upsertSearchEvent(events: ArtifactEvent[] | undefined, search: SearchCall): ArtifactEvent[] {
  const out = [...(events ?? [])];
  const id = searchKey(search);
  const idx = out.findIndex((event) => event.kind === "search" && event.id === id);
  if (idx >= 0) {
    out[idx] = { id, kind: "search", search };
    return out;
  }
  if (search.status === "completed") {
    const pendingIdx = out.findIndex((event) =>
      event.kind === "search" &&
      event.search?.status === "in_progress" &&
      (event.search.action === search.action || event.search.action === "unknown" || event.search.detail === search.detail)
    );
    if (pendingIdx >= 0) {
      out[pendingIdx] = { id, kind: "search", search };
      return out;
    }
  }
  out.push({ id, kind: "search", search });
  return out;
}

function appendNarrationEvent(
  events: ArtifactEvent[] | undefined,
  narration: NarrationStep,
  eventId: string,
): ArtifactEvent[] {
  const text = normalizeNarrationText(narration.text);
  if (!text) return [...(events ?? [])];
  if ((events ?? []).some((event) =>
    event.kind === "narration" &&
    normalizeNarrationText(event.narration?.text ?? "") === text
  )) {
    return [...(events ?? [])];
  }
  return [...(events ?? []), { id: eventId, kind: "narration", narration }];
}

function normalizeNarrationText(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

function appendNarrationStep(steps: NarrationStep[] | undefined, step: NarrationStep): NarrationStep[] {
  const out = [...(steps ?? [])];
  const text = normalizeNarrationText(step.text);
  if (!text) return out;
  if (out.some((existing) => normalizeNarrationText(existing.text) === text)) return out;
  out.push(step);
  return out;
}

function isFatalStopReason(stopReason: string | undefined): boolean {
  return stopReason === "max_tool_turns" ||
    stopReason === "max_tool_turns_finalized" ||
    stopReason === "user_aborted" ||
    stopReason === "provider_error" ||
    stopReason === "provider_stream_error" ||
    stopReason === "provider_stream_idle_timeout" ||
    stopReason === "provider_synthesis_timeout" ||
    stopReason === "agent_error";
}

function mergeArtifactEvents(to: ArtifactEvent[] | undefined, from: ArtifactEvent[] | undefined): ArtifactEvent[] {
  const out = [...(to ?? [])];
  for (const event of from ?? []) {
    if (
      event.kind === "narration" &&
      out.some((existing) =>
        existing.kind === "narration" &&
        normalizeNarrationText(existing.narration?.text ?? "") === normalizeNarrationText(event.narration?.text ?? "")
      )
    ) {
      continue;
    }
    const idx = out.findIndex((existing) => existing.id === event.id && existing.kind === event.kind);
    if (event.kind === "tool" && event.tool) {
      const merged = upsertToolEvent(out, event.tool, event.turn);
      out.splice(0, out.length, ...merged);
    } else if (idx >= 0) out[idx] = { ...out[idx], ...event };
    else out.push(event);
  }
  return out;
}

function isCanvasTool(t: ToolCall): boolean {
  return t.name !== "update_plan";
}

function isCanvasArtifact(event: ArtifactEvent): boolean {
  return event.kind !== "tool" || !event.tool || isCanvasTool(event.tool);
}

function parseClarificationPayload(p: any): ClarificationPayload | null {
  const questions = Array.isArray(p?.questions)
    ? p.questions
        .map((q: any) => ({
          id: String(q.id ?? "").trim(),
          header: String(q.header ?? "").trim(),
          question: String(q.question ?? "").trim(),
          options: Array.isArray(q.options)
            ? q.options
                .map((o: any) => ({
                  label: String(o.label ?? "").trim(),
                  description: String(o.description ?? "").trim(),
                }))
                .filter((o: ClarificationOption) => o.label && o.description)
            : [],
        }))
        .filter((q: ClarificationQuestion) => q.id && q.question && q.options.length > 0)
    : [];
  const fallbackQuestion = String(p?.question ?? "").trim();
  if (questions.length === 0 && !fallbackQuestion) return null;
  return {
    question: fallbackQuestion || questions.map((q: ClarificationQuestion) => q.question).join("\n"),
    questions,
  };
}

function parsePlanPayload(p: any): AgentPlanView | null {
  if (!Array.isArray(p?.items)) return null;
  const items = p.items
    .map((item: any) => ({
      id: typeof item.id === "string" ? item.id : typeof item.item_id === "string" ? item.item_id : undefined,
      seq: typeof item.seq === "number" ? item.seq : undefined,
      step: String(item.step ?? "").trim(),
      status: String(item.status ?? "pending"),
      evidence: typeof item.evidence === "string" && item.evidence.trim() ? item.evidence.trim() : null,
    }))
    .filter((item: any) => item.step);
  if (items.length === 0) return null;
  return {
    task_id: typeof p.task_id === "string" ? p.task_id : null,
    explanation: typeof p.explanation === "string" && p.explanation.trim() ? p.explanation.trim() : null,
    items,
  };
}

function ClarificationRequestCard(props: {
  payload: ClarificationPayload;
  onPick: (text: string) => void;
}) {
  const questions = () => props.payload.questions.length > 0
    ? props.payload.questions
    : [{
        id: "clarification",
        header: "clarify",
        question: props.payload.question,
        options: [],
      }];

  function answerText(question: ClarificationQuestion, option: ClarificationOption) {
    return `关于“${question.question}”，我的选择是：${option.label}。${option.description}`;
  }

  return (
    <div class="lk-clarify live">
      <For each={questions()}>{(q) => (
        <div class="lk-clarify-block">
          <div class="lk-clarify-q">
            <b>{q.header || "L.E.E.K asks"} ·</b> {q.question}
          </div>
          <Show
            when={q.options.length > 0}
            fallback={<div class="lk-clarify-free">直接在输入框回复也可以。</div>}
          >
            <div class="lk-clarify-opts column">
              <For each={q.options}>{(o) => (
                <button
                  type="button"
                  class="lk-clarify-option"
                  onClick={() => props.onPick(answerText(q, o))}
                >
                  <span>{o.label}</span>
                  <em>{o.description}</em>
                </button>
              )}</For>
            </div>
          </Show>
        </div>
      )}</For>
    </div>
  );
}

interface UsageInfo {
  inTokens: number;
  outTokens: number;
}

interface ProviderRetryNotice {
  retry: number;
  max: number;
  delaySec: number;
  message: string;
}

function fmtTime(d: Date | string = new Date()): string {
  // Render HH:MM in the user's local timezone. RFC3339 strings (UTC) get
  // converted via Date(); Date instances pass through.
  const date = typeof d === "string" ? new Date(d) : d;
  if (Number.isNaN(date.getTime())) return "";
  const h = String(date.getHours()).padStart(2, "0");
  const m = String(date.getMinutes()).padStart(2, "0");
  return `${h}:${m}`;
}

interface LiveTick {
  seq: number;
  kind: string;
  payload: unknown;
  ts: string;
}

export function LiveChat(props: { onNavigate?: (page: "chat" | "portfolio" | "settings") => void } = {}) {
  const [sessionId, setSessionId] = createSignal<string>(readSessionFromLocation());
  const [sessions, setSessions] = createSignal<SessionRow[]>([]);
  const [messages, setMessages] = createSignal<LiveMsg[]>([]);
  const [usage, setUsage] = createSignal<UsageInfo | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [providerRetry, setProviderRetry] = createSignal<ProviderRetryNotice | null>(null);
  const [pending, setPending] = createSignal(false);
  const [stopping, setStopping] = createSignal(false);
  const [connected, setConnected] = createSignal(false);
  // Compaction state — set when /compact is in flight. While true, the
  // composer locks SEND, new submits queue (don't fire), and Esc routes to
  // the same /abort endpoint normal replies use.
  const [compacting, setCompacting] = createSignal(false);
  const [compactQueue, setCompactQueue] = createSignal<string[]>([]);
  const [pendingCompactionTargetId, setPendingCompactionTargetId] = createSignal<string | null>(null);
  // Trigger of the in-flight compaction (`manual` | `auto_pre_turn`). Used
  // to gate the Composer's STOP button: auto compactions cannot be cancelled.
  const [compactionTrigger, setCompactionTrigger] = createSignal<string | null>(null);
  const [eventsOpen, setEventsOpen] = createSignal(false);
  const [liveTick, setLiveTick] = createSignal<LiveTick | null>(null);
  const [agentPlan, setAgentPlan] = createSignal<AgentPlanView | null>(null);
  // Wall-clock seconds since the current agent reply started. Drives the
  // "thinking · 24s" status row above the streaming message.
  const [elapsedSec, setElapsedSec] = createSignal(0);
  // Wiki preview modal — pop-able from brain node clicks AND corpus_search
  // hit tile clicks AND in-modal wikilinks. Lives at this level so all three
  // entry points share one modal stack.
  const [openDoc, setOpenDoc] = createSignal<CorpusDoc | null>(null);
  const [docLoading, setDocLoading] = createSignal(false);
  const [docError, setDocError] = createSignal<string | null>(null);

  async function openWiki(id: string, fallbackTitle = "") {
    setDocLoading(true);
    setDocError(null);
    setOpenDoc({ id, title: fallbackTitle, tier: "", layer: "", tags: [], body: "" });
    try {
      const r = await fetch(`/api/v1/corpus/doc?id=${encodeURIComponent(id)}`);
      if (!r.ok) {
        setDocError(`HTTP ${r.status}`);
        return;
      }
      const doc: CorpusDoc = await r.json();
      setOpenDoc(doc);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : "network error";
      setDocError(msg);
    } finally {
      setDocLoading(false);
    }
  }
  function closeWiki() { setOpenDoc(null); setDocError(null); }

  createEffect(() => {
    if (!openDoc()) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeWiki();
    };
    document.addEventListener("keydown", handler);
    onCleanup(() => document.removeEventListener("keydown", handler));
  });

  let evtSrc: EventSource | undefined;
  let agentBuffer = "";
  let chatScrollEl: HTMLDivElement | undefined;
  let agentStartTs = 0;
  let elapsedTimer: number | undefined;

  async function refreshSessions() {
    try {
      const r = await fetch("/api/v1/sessions");
      if (r.ok) {
        const j = await r.json();
        setSessions(j.items ?? []);
      }
    } catch {/* ignore */}
  }

  async function createSession() {
    try {
      const r = await fetch("/api/v1/sessions", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title: "untitled session" }),
      });
      if (!r.ok) return;
      const j = await r.json();
      await refreshSessions();
      switchSession(j.id);
    } catch {/* ignore */}
  }

  async function renameSession(id: string, title: string) {
    const nextTitle = title.trim();
    if (!nextTitle) return;
    setSessions((prev) => prev.map((s) => s.id === id ? { ...s, title: nextTitle } : s));
    try {
      const r = await fetch(`/api/v1/sessions/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title: nextTitle }),
      });
      if (!r.ok) throw new Error(`rename failed: ${r.status}`);
      await refreshSessions();
    } catch {
      await refreshSessions();
    }
  }

  async function deleteSession(id: string) {
    try {
      await fetch(`/api/v1/sessions/${id}`, { method: "DELETE" });
      await refreshSessions();
      if (id === sessionId()) {
        // Falling back to the default session keeps the UI in a known state.
        switchSession(DEFAULT_SESSION_ID);
      }
    } catch {/* ignore */}
  }

  function emitTick(e: MessageEvent, kind: string, payload: unknown) {
    // The SSE `id` field carries the backend-assigned vault.events.seq —
    // EventsPanel dedupes against that so live ticks merge cleanly with
    // history reloads.
    const seq = parseInt(e.lastEventId, 10);
    if (!Number.isFinite(seq)) return;
    setLiveTick({ seq, kind, payload, ts: new Date().toISOString() });
  }

  function clearProviderRetry() {
    if (providerRetry()) setProviderRetry(null);
  }

  // Auto-scroll to bottom whenever messages change (length OR last text changes
  // during streaming). Without this, new replies render below the viewport.
  createEffect(() => {
    const list = messages();
    if (list.length > 0) {
      // touch the last message's text so the effect re-runs on every delta
      void list[list.length - 1].text;
    }
    if (chatScrollEl) {
      chatScrollEl.scrollTop = chatScrollEl.scrollHeight;
    }
  });

  function appendDelta(text: string) {
    agentBuffer += text;
    setMessages((prev) => {
      const out = [...prev];
      const last = out[out.length - 1];
      if (last && last.role === "agent" && last.streaming) {
        out[out.length - 1] = { ...last, text: agentBuffer };
      }
      return out;
    });
  }

  async function connect(id: string) {
    // 1. Load message + event history. Messages give us the chat backbone
    //    (text, role, ts); events give us the tool_call / web_search /
    //    narration trail we want to keep visible across reloads.
    setMessages([]);
    setUsage(null);
    setPending(false);
    setAgentPlan(null);

    let hist: LiveMsg[] = [];
    try {
      const r = await fetch(`/api/v1/sessions/${id}/messages?limit=200`);
      if (r.ok) {
        const json = await r.json();
        hist = (json.items ?? []).map((m: any) => {
          let text = "";
          try { text = JSON.parse(m.content_json).text ?? ""; } catch {/* ignore */}
          const role: LiveMsg["role"] =
            m.role === "agent" ? "agent"
            : m.role === "compaction_summary" ? "compaction_summary"
            : "user";
          return {
            role,
            text,
            ts: fmtTime(m.created_at),
            msg_seq: typeof m.seq === "number" ? m.seq : undefined,
            raw_ts: m.created_at,
            searches: [],
            tool_calls: [],
            narrations: [],
            artifacts: [],
          } as LiveMsg;
        });
      }
    } catch {/* history is optional */}

    // 2. Replay events into agent messages so tool calls / searches /
    //    narrations are visible after reload.
    try {
      const r = await fetch(`/api/v1/sessions/${id}/events?limit=5000`);
      if (r.ok) {
        const ev = await r.json();
        const agentIdxs: number[] = [];
        let cursor = -1; // index into event-created agent slots
        let usageSnap: UsageInfo | null = null;
        let lastSec: number | undefined;
        let planSnap: AgentPlanView | null = null;
        const mergeAgentActivity = (fromIdx: number, toIdx: number) => {
          const from = hist[fromIdx];
          const to = hist[toIdx];
          to.searches = [...(to.searches ?? []), ...(from.searches ?? [])];
          const byCall = new Map<string, ToolCall>();
          for (const t of [...(to.tool_calls ?? []), ...(from.tool_calls ?? [])]) {
            const prev = byCall.get(t.call_id);
            byCall.set(t.call_id, { ...(prev ?? t), ...t, arguments: prev?.arguments ?? t.arguments });
          }
          to.tool_calls = Array.from(byCall.values());
          let narrations = to.narrations ?? [];
          for (const step of from.narrations ?? []) narrations = appendNarrationStep(narrations, step);
          to.narrations = narrations;
          to.artifacts = mergeArtifactEvents(to.artifacts, from.artifacts);
          to.clarification = from.clarification ?? to.clarification;
          to.total_sec = from.total_sec ?? to.total_sec;
          from.hidden = true;
        };
        for (const row of ev.items ?? []) {
          const p = (() => { try { return JSON.parse(row.payload_json); } catch { return {}; } })();
          switch (row.kind) {
            case "agent_message_start":
              cursor++;
              hist.push({
                role: "agent",
                text: "",
                ts: fmtTime(row.ts ?? row.created_at),
                raw_ts: row.ts ?? row.created_at,
                searches: [],
                tool_calls: [],
                narrations: [],
                artifacts: [],
              });
              agentIdxs.push(hist.length - 1);
              break;
            case "agent_message_end":
              if (cursor >= 0 && cursor < agentIdxs.length) {
                const currentIdx = agentIdxs[cursor];
                if (typeof lastSec === "number") hist[currentIdx].total_sec = lastSec;
                if (typeof p.message_seq === "number") {
                  const targetIdx = hist.findIndex((m) => m.role === "agent" && m.msg_seq === p.message_seq);
                  if (targetIdx >= 0 && targetIdx !== currentIdx) mergeAgentActivity(currentIdx, targetIdx);
                }
              }
              break;
            case "tool_call":
              if (cursor >= 0 && cursor < agentIdxs.length) {
                const msg = hist[agentIdxs[cursor]];
                const next = toolCallFromPayload(p);
                msg.tool_calls = upsertToolCall(msg.tool_calls, next);
                msg.artifacts = upsertToolEvent(msg.artifacts, next);
              }
              break;
            case "web_search_call":
              if (cursor >= 0 && cursor < agentIdxs.length) {
                const msg = hist[agentIdxs[cursor]];
                const searches = (msg.searches ?? []) as SearchCall[];
                const sc: SearchCall = {
                  status: p.status ?? "in_progress",
                  action: p.action ?? "unknown",
                  detail: p.detail ?? "",
                  queries: Array.isArray(p.queries)
                    ? p.queries.filter((q: unknown): q is string => typeof q === "string")
                    : undefined,
                  sources: Array.isArray(p.sources)
                    ? p.sources.filter((s: unknown): s is string => typeof s === "string")
                    : undefined,
                };
                if (sc.status === "completed") {
                  const idx2 = searches.findIndex((s) =>
                    s.status === "in_progress" &&
                    (s.action === sc.action || s.action === "unknown" || s.detail === sc.detail));
                  if (idx2 >= 0) searches[idx2] = sc;
                  else searches.push(sc);
                } else {
                  searches.push(sc);
                }
                msg.searches = searches;
                msg.artifacts = upsertSearchEvent(msg.artifacts, sc);
              }
              break;
            case "agent_narration":
              if (cursor >= 0 && cursor < agentIdxs.length) {
                const msg = hist[agentIdxs[cursor]];
                const step = { turn: typeof p.turn === "number" ? p.turn : 0, text: String(p.text ?? "") };
                const ns = appendNarrationStep(msg.narrations, step);
                if (ns.length === (msg.narrations?.length ?? 0)) break;
                msg.narrations = ns;
                msg.artifacts = appendNarrationEvent(msg.artifacts, step, `narration:${row.seq ?? row.id ?? ns.length}:${ns.length}`);
              }
              break;
            case "clarification_requested": {
              const clarification = parseClarificationPayload(p);
              if (!clarification) break;
              let idx = cursor >= 0 && cursor < agentIdxs.length ? agentIdxs[cursor] : -1;
              if (idx < 0) {
                for (let i = hist.length - 1; i >= 0; i--) {
                  if (hist[i].role === "agent") {
                    idx = i;
                    break;
                  }
                }
              }
              if (idx >= 0) hist[idx].clarification = clarification;
              break;
            }
            case "plan_updated": {
              const nextPlan = parsePlanPayload(p);
              if (nextPlan) planSnap = nextPlan;
              break;
            }
            case "llm_usage":
              usageSnap = {
                inTokens: typeof p.input_tokens === "number" ? p.input_tokens : 0,
                outTokens: typeof p.output_tokens === "number" ? p.output_tokens : 0,
              };
              break;
            case "decision_draft_ready":
              hist.push({
                role: "decision_draft",
                text: "",
                ts: fmtTime(row.ts ?? row.created_at),
                raw_ts: row.ts ?? row.created_at,
                decision_draft: p as DecisionDraftPayload,
              });
              break;
          }
        }
        if (usageSnap) setUsage(usageSnap);
        if (planSnap) setAgentPlan(planSnap);

        // Restore compaction state: if the most recent compaction.started has
        // no matching completed/aborted after it, compaction is still running.
        let lastStartTs = "";
        let lastEndTs = "";
        for (const row of ev.items ?? []) {
          const rowTs = row.ts ?? row.created_at ?? "";
          if (row.kind === "compaction.started") lastStartTs = rowTs;
          if (row.kind === "compaction.completed" || row.kind === "compaction.aborted") {
            lastEndTs = rowTs;
          }
        }
        if (lastStartTs && lastStartTs > lastEndTs) {
          setCompacting(true);
        }
      }
    } catch {/* events optional */}

    hist = hist
      .filter((m) => !m.hidden)
      .sort((a, b) => (a.raw_ts ?? "").localeCompare(b.raw_ts ?? ""));
    setMessages(hist);

    // 3. Subscribe to live event stream
    evtSrc = new EventSource(`/stream/sessions/${id}/events`);

    evtSrc.addEventListener("open", () => {
      setConnected(true);
      setError((prev) => prev === "stream reconnecting…" ? null : prev);
    });
    evtSrc.onerror = () => {
      setConnected(false);
      // EventSource auto-reconnects; surface a soft hint
      setError("stream reconnecting…");
    };

    // Server echoes user_message — we already added it optimistically on send,
    // so we dedupe in the chat view but still forward to EventsPanel.
    evtSrc.addEventListener("user_message", (e: MessageEvent) => {
      try { emitTick(e, "user_message", JSON.parse(e.data)); } catch { /* skip */ }
    });

    evtSrc.addEventListener("agent_message_start", (e: MessageEvent) => {
      agentBuffer = "";
      setPending(true);
      setStopping(false);
      // send() already inserted a streaming placeholder when the user hit
      // Enter (so "thinking · Ns" appears instantly). If we got here on a
      // server-initiated reply (e.g. resumed session), insert one now.
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (last && last.role === "agent" && last.streaming) return prev;
        return [
          ...prev,
          { role: "agent", text: "", ts: fmtTime(), streaming: true, searches: [], tool_calls: [], narrations: [], artifacts: [] },
        ];
      });
      if (!elapsedTimer) {
        agentStartTs = Date.now();
        setElapsedSec(0);
        elapsedTimer = window.setInterval(() => {
          setElapsedSec(Math.max(0, Math.floor((Date.now() - agentStartTs) / 1000)));
        }, 1000);
      }
      try { emitTick(e, "agent_message_start", JSON.parse(e.data)); } catch { /* skip */ }
    });

    evtSrc.addEventListener("tool_call", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        clearProviderRetry();
        emitTick(e, "tool_call", data);
        const call = toolCallFromPayload(data);
        setMessages((prev) => {
          const out = [...prev];
          const last = out[out.length - 1];
          if (!last || last.role !== "agent" || !last.streaming) return prev;
          const tool_calls = upsertToolCall(last.tool_calls, call);
          out[out.length - 1] = { ...last, tool_calls, artifacts: upsertToolEvent(last.artifacts, call) };
          return out;
        });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("web_search_call", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        clearProviderRetry();
        emitTick(e, "web_search_call", data);
        const call: SearchCall = {
          status: data.status ?? "in_progress",
          action: data.action ?? "unknown",
          detail: data.detail ?? "",
          queries: Array.isArray(data.queries)
            ? data.queries.filter((q: unknown): q is string => typeof q === "string")
            : undefined,
          sources: Array.isArray(data.sources)
            ? data.sources.filter((s: unknown): s is string => typeof s === "string")
            : undefined,
        };
        setMessages((prev) => {
          const out = [...prev];
          const last = out[out.length - 1];
          if (!last || last.role !== "agent" || !last.streaming) return prev;
          const searches = [...(last.searches ?? [])];
          // Match completed -> in_progress by (action, detail) so the same
          // chip flips state instead of duplicating. detail can be empty on
          // in_progress (codex emits action only on done) — fall back to
          // updating the last chip in that case.
          if (call.status === "completed") {
            const idx = searches.findIndex(
              (s) => s.status === "in_progress" &&
                     (s.action === call.action || s.action === "unknown" || call.detail === s.detail)
            );
            if (idx >= 0) searches[idx] = call;
            else searches.push(call);
          } else {
            searches.push(call);
          }
          out[out.length - 1] = { ...last, searches, artifacts: upsertSearchEvent(last.artifacts, call) };
          return out;
        });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("agent_message_delta", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        clearProviderRetry();
        if (typeof data.text === "string") appendDelta(data.text);
        emitTick(e, "agent_message_delta", data);
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("agent_message_reset", (e: MessageEvent) => {
      agentBuffer = "";
      setMessages((prev) => {
        const out = [...prev];
        const last = out[out.length - 1];
        if (!last || last.role !== "agent" || !last.streaming) return prev;
        out[out.length - 1] = { ...last, text: "" };
        return out;
      });
      try { emitTick(e, "agent_message_reset", JSON.parse(e.data)); } catch { /* skip */ }
    });

    evtSrc.addEventListener("agent_narration", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        emitTick(e, "agent_narration", data);
        const step: NarrationStep = {
          turn: typeof data.turn === "number" ? data.turn : 0,
          text: String(data.text ?? ""),
        };
        setMessages((prev) => {
          const out = [...prev];
          const last = out[out.length - 1];
          if (!last || last.role !== "agent" || !last.streaming) return prev;
          const narrations = appendNarrationStep(last.narrations, step);
          if (narrations.length === (last.narrations?.length ?? 0)) return prev;
          const eventId = `narration:${e.lastEventId || Date.now()}:${narrations.length}`;
          out[out.length - 1] = {
            ...last,
            narrations,
            artifacts: appendNarrationEvent(last.artifacts, step, eventId),
          };
          return out;
        });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("plan_updated", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        emitTick(e, "plan_updated", data);
        const nextPlan = parsePlanPayload(data);
        if (nextPlan) setAgentPlan(nextPlan);
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("llm_usage", (e: MessageEvent) => {
      try {
        const u = JSON.parse(e.data);
        clearProviderRetry();
        setUsage({ inTokens: u.input_tokens ?? 0, outTokens: u.output_tokens ?? 0 });
        emitTick(e, "llm_usage", u);
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("provider_retry", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        const retry = Number(data.retry ?? 0);
        const max = Number(data.max_retries ?? 10);
        const delaySec = Math.max(0, Math.round(Number(data.delay_ms ?? 0) / 1000));
        setProviderRetry({
          retry,
          max,
          delaySec,
          message: typeof data.message === "string" ? data.message : "provider temporarily unavailable",
        });
        emitTick(e, "provider_retry", data);
      } catch {
        setProviderRetry({
          retry: 0,
          max: 10,
          delaySec: 0,
          message: "provider temporarily unavailable",
        });
      }
    });

    evtSrc.addEventListener("agent_message_end", (e: MessageEvent) => {
      let data: any = {};
      try {
        data = JSON.parse(e.data);
      } catch {
        data = {};
      }
      const stopReason = String(data.stop_reason ?? "");
      const wasStopping = stopping();
      setPending(false);
      setStopping(false);
      clearProviderRetry();
      if (!isFatalStopReason(stopReason) || (stopReason === "user_aborted" && wasStopping)) setError(null);
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = undefined;
      }
      const finalSec = agentStartTs ? Math.max(0, Math.floor((Date.now() - agentStartTs) / 1000)) : 0;
      setMessages((prev) => {
        const out = [...prev];
        const last = out[out.length - 1];
        if (last && last.role === "agent") {
          // Freeze final elapsed time on the message itself so it stays
          // visible after streaming ends (the live signal resets to 0).
          out[out.length - 1] = { ...last, streaming: false, total_sec: finalSec };
        }
        return out;
      });
      setElapsedSec(0);
      emitTick(e, "agent_message_end", data);
    });

    evtSrc.addEventListener("decision_draft_ready", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data) as DecisionDraftPayload;
        setMessages((prev) => [
          ...prev,
          { role: "decision_draft", text: "", ts: fmtTime(), decision_draft: data },
        ]);
        try { emitTick(e, "decision_draft_ready", data); } catch { /* skip */ }
      } catch { /* skip */ }
    });

    evtSrc.addEventListener("clarification_requested", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        emitTick(e, "clarification_requested", data);
        const clarification = parseClarificationPayload(data);
        if (!clarification) return;
        setMessages((prev) => {
          const out = [...prev];
          const last = out[out.length - 1];
          if (!last || last.role !== "agent" || !last.streaming) return prev;
          out[out.length - 1] = { ...last, clarification };
          return out;
        });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("task_started", (e: MessageEvent) => {
      setAgentPlan(null);
      try { emitTick(e, "task_started", JSON.parse(e.data)); } catch { /* skip */ }
    });
    evtSrc.addEventListener("task_created", (e: MessageEvent) => {
      try { emitTick(e, "task_created", JSON.parse(e.data)); } catch { /* skip */ }
    });
    evtSrc.addEventListener("task_delivered", (e: MessageEvent) => {
      try { emitTick(e, "task_delivered", JSON.parse(e.data)); } catch { /* skip */ }
    });
    evtSrc.addEventListener("task_awaiting_user", (e: MessageEvent) => {
      try { emitTick(e, "task_awaiting_user", JSON.parse(e.data)); } catch { /* skip */ }
    });
    evtSrc.addEventListener("task_failed", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        setError(data.reason ?? "task failed");
        emitTick(e, "task_failed", data);
      } catch {
        setError("task failed");
      }
    });

    // Task lifecycle is audit data; the right sidebar only keeps the active
    // plan, so a new task clears the prior completed checklist until the next
    // update_plan arrives.

    // Compaction lifecycle. Backend emits `started` synchronously when the
    // POST handler runs, then `completed` (success) or `aborted` (Esc /
    // failure) from the spawned task. Frontend mirrors these into UI lock
    // state and (on completion) navigates to the new session.
    evtSrc.addEventListener("compaction.started", (e: MessageEvent) => {
      setCompacting(true);
      try {
        const data = JSON.parse(e.data) as { trigger?: string };
        setCompactionTrigger(data.trigger ?? null);
      } catch {
        setCompactionTrigger(null);
      }
    });
    evtSrc.addEventListener("compaction.completed", async () => {
      // Reload the same session — compaction_summary is now appended in-place.
      // Reconnect so the SSE stream reattaches after connect() closes the old one.
      evtSrc?.close();
      evtSrc = undefined;
      await connect(sessionId());
      // Clear compacting flag BEFORE flushing the queue so send() POSTs instead
      // of re-queuing.
      setCompacting(false);
      setCompactionTrigger(null);
      setPendingCompactionTargetId(null);
      const queued = compactQueue();
      setCompactQueue([]);
      for (const text of queued) {
        await send(text);
      }
    });
    evtSrc.addEventListener("compaction.aborted", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data) as { reason?: string };
        setError(`compaction aborted: ${data.reason ?? "unknown"}`);
      } catch {
        setError("compaction aborted");
      }
      setCompacting(false);
      setCompactionTrigger(null);
      setPendingCompactionTargetId(null);
    });

    evtSrc.addEventListener("error", (e: MessageEvent) => {
      clearProviderRetry();
      try {
        const data = JSON.parse(e.data);
        setError(data.message ?? "agent error");
      } catch {
        setError("agent error");
      }
      setPending(false);
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = undefined;
      }
      setElapsedSec(0);
      // Finalize any hanging streaming bubble so it doesn't spin forever.
      const finalSec = agentStartTs ? Math.max(0, Math.floor((Date.now() - agentStartTs) / 1000)) : 0;
      setMessages((prev) => {
        const last = prev[prev.length - 1];
        if (!last || last.role !== "agent" || !last.streaming) return prev;
        const hasActivity =
          (last.searches?.length ?? 0) > 0 ||
          (last.tool_calls?.length ?? 0) > 0 ||
          (last.narrations?.length ?? 0) > 0 ||
          (last.artifacts?.length ?? 0) > 0;
        if (!last.text && !hasActivity) return prev.slice(0, -1); // empty bubble → remove
        const out = [...prev];
        out[out.length - 1] = { ...last, streaming: false, total_sec: finalSec };
        return out;
      });
    });
  }

  async function switchSession(id: string) {
    if (id === sessionId()) return;
    evtSrc?.close();
    evtSrc = undefined;
    setSessionId(id);
    writeSessionToLocation(id);
    await connect(id);
  }

  onMount(() => {
    void connect(sessionId());
    void refreshSessions();
    const onLocation = () => {
      const next = readSessionFromLocation();
      if (next !== sessionId()) switchSession(next);
    };
    window.addEventListener("hashchange", onLocation);
    window.addEventListener("popstate", onLocation);
    // Restore the chat column width preference saved last drag session.
    const saved = localStorage.getItem("lk-chat-col");
    if (saved && /^\d+px$/.test(saved)) {
      document.documentElement.style.setProperty("--lk-chat-col", saved);
    }
    onCleanup(() => {
      window.removeEventListener("hashchange", onLocation);
      window.removeEventListener("popstate", onLocation);
    });
  });

  // Drag the chat ↔ canvas seam. Bound at run-time on mousedown so we don't
  // pay listener overhead while idle.
  function startResize(e: MouseEvent) {
    e.preventDefault();
    const target = e.currentTarget as HTMLElement;
    target.classList.add("dragging");
    const railWidth = 56; // .lk-app rail column width
    const onMove = (ev: MouseEvent) => {
      const x = Math.max(280, Math.min(window.innerWidth - 360, ev.clientX - railWidth));
      document.documentElement.style.setProperty("--lk-chat-col", `${x}px`);
    };
    const onUp = () => {
      target.classList.remove("dragging");
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      const v = document.documentElement.style.getPropertyValue("--lk-chat-col");
      if (v) localStorage.setItem("lk-chat-col", v.trim());
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  onCleanup(() => {
    evtSrc?.close();
    if (elapsedTimer) clearInterval(elapsedTimer);
  });

  // Cmd+E / Ctrl+E toggles the events timeline drawer.
  createEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "e") {
        e.preventDefault();
        setEventsOpen((v) => !v);
      }
    };
    document.addEventListener("keydown", handler);
    onCleanup(() => document.removeEventListener("keydown", handler));
  });

  async function send(text: string) {
    setError(null);
    setStopping(false);
    setAgentPlan(null);
    // While a compaction is in flight, hold the message in a local queue
    // rather than firing it. The compaction.completed handler flushes the
    // queue into the new session in order. Modeled on Claude Code's
    // "messages enter the queue while compaction runs" behavior.
    if (compacting()) {
      setCompactQueue((q) => [...q, text]);
      return;
    }
    // Optimistic user message + immediate thinking placeholder so the
    // user sees feedback within a frame instead of waiting for the
    // backend's first SSE event (cold codex token refresh / network can
    // burn 2-4s).
    setMessages((prev) => [
      ...prev,
      { role: "user", text, ts: fmtTime() },
      { role: "agent", text: "", ts: fmtTime(), streaming: true, searches: [], tool_calls: [], narrations: [], artifacts: [] },
    ]);
    setPending(true);
    agentStartTs = Date.now();
    setElapsedSec(0);
    if (elapsedTimer) clearInterval(elapsedTimer);
    elapsedTimer = window.setInterval(() => {
      setElapsedSec(Math.max(0, Math.floor((Date.now() - agentStartTs) / 1000)));
    }, 1000);
    const finishFailedSend = (message: string) => {
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = undefined;
      }
      setPending(false);
      setStopping(false);
      setElapsedSec(0);
      setError(message);
      setMessages((prev) => {
        const out = [...prev];
        const last = out[out.length - 1];
        if (last?.role === "agent" && last.streaming && !last.text && !(last.tool_calls?.length) && !(last.searches?.length)) {
          out.pop();
        } else if (last?.role === "agent" && last.streaming) {
          out[out.length - 1] = { ...last, streaming: false };
        }
        return out;
      });
    };
    try {
      const r = await fetch(`/api/v1/sessions/${sessionId()}/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ content: { type: "text", text } }),
      });
      if (r.status === 202) {
        // Auto-compaction kicked off pre-turn: backend hasn't persisted our
        // user message and won't start a reply. Roll back the optimistic UI
        // and queue the text for re-send against the new session once
        // `compaction.completed` SSE arrives.
        const body = (await r.json().catch(() => ({}))) as {
          auto_compacting?: boolean;
        };
        if (body.auto_compacting) {
          setMessages((prev) => prev.slice(0, -2));
          setPending(false);
          if (elapsedTimer) {
            clearInterval(elapsedTimer);
            elapsedTimer = undefined;
          }
          setCompactQueue((q) => [...q, text]);
          // SSE `compaction.started` will flip `compacting` on its own; the
          // flush happens in the `compaction.completed` handler.
          return;
        }
      }
      if (!r.ok) {
        const body = await r.text();
        finishFailedSend(`POST ${r.status}: ${body.slice(0, 200)}`);
      }
    } catch (e: any) {
      finishFailedSend(e?.message ?? "network error");
    }
  }

  async function stop() {
    if (!pending() && !compacting()) return;
    setStopping(true);
    try {
      const r = await fetch(`/api/v1/sessions/${sessionId()}/abort`, { method: "POST" });
      if (!r.ok) {
        const body = await r.text();
        setStopping(false);
        setError(`abort ${r.status}: ${body.slice(0, 200)}`);
        return;
      }
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = undefined;
      }
      const finalSec = agentStartTs ? Math.max(0, Math.floor((Date.now() - agentStartTs) / 1000)) : 0;
      setPending(false);
      setStopping(false);
      setCompacting(false);
      setElapsedSec(0);
      setError(null);
      setMessages((prev) => {
        const out = [...prev];
        const last = out[out.length - 1];
        if (last && last.role === "agent" && last.streaming) {
          out[out.length - 1] = { ...last, streaming: false, total_sec: finalSec };
        }
        return out;
      });
    } catch (e: any) {
      setStopping(false);
      setError(e?.message ?? "abort failed");
    }
  }

  // Slash menu actions, exposed both as visible chips in the composer row and
  // through typing `/` to filter. /clear clears the current view (not backend
  // history — reload restores it). /compact is a placeholder for the future
  // LLM-side conversation summarisation step. (New-session is already in the
  // SessionMenu dropdown so it doesn't need a slash entry.)
  const slashCommands: SlashCommand[] = [
    {
      name: "clear",
      hint: "清空当前对话视图",
      run: () => {
        setMessages([]);
        setUsage(null);
        setError(null);
      },
    },
    {
      name: "compact",
      hint: "压缩当前对话上下文",
      run: () => {
        if (compacting() || pending()) return;
        setError(null);
        setCompacting(true);
        void (async () => {
          try {
            const r = await fetch(`/api/v1/sessions/${sessionId()}/compact`, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ trigger: "manual" }),
            });
            if (!r.ok) {
              setCompacting(false);
              const body = await r.text();
              setError(`compact ${r.status}: ${body.slice(0, 200)}`);
              return;
            }
            // compaction.completed SSE will reload messages and flush queue.
          } catch (e: any) {
            setCompacting(false);
            setError(e?.message ?? "compact failed");
          }
        })();
      },
    },
  ];

  // Derive a Scene from real session state so the Workbench shell (canvas
  // header / brain meta / etc) shows the right form. `idle` until a turn
  // happens; `thinking-shallow` while pending; `delivered` once a reply
  // exists. `clarify` / `deep` are reserved for future clarification and
  // active multi-turn task states.
  const derivedScene = createMemo<Scene>(() => {
    if (pending()) return "thinking-shallow";
    if (messages().some((m) => m.role === "agent")) return "delivered";
    return "idle";
  });

  // Aggregate every tool call / web_search across the whole session so the
  // canvas keeps the trail of artifacts visible — turn boundaries don't
  // erase prior research. Cleared by /clear (planned).
  const sessionTools = createMemo<ToolCall[]>(() => {
    const out: ToolCall[] = [];
    for (const m of messages()) {
      if (m.role !== "agent" || !m.tool_calls) continue;
      for (const t of m.tool_calls) {
        out.splice(0, out.length, ...upsertToolCall(out, t));
      }
    }
    return out;
  });
  const visibleSessionTools = createMemo<ToolCall[]>(() =>
    sessionTools().filter(isCanvasTool)
  );
  const sessionSearches = createMemo<SearchCall[]>(() => {
    const out: SearchCall[] = [];
    for (const m of messages()) {
      if (m.role !== "agent" || !m.searches) continue;
      for (const s of m.searches) out.push(s);
    }
    return out;
  });

  // Harvest page titles from web_fetch tool calls so chat-side plain URL
  // autolinks render as the real page title instead of just a hostname.
  const urlTitles = createMemo<Record<string, string>>(() => {
    const out: Record<string, string> = {};
    for (const t of sessionTools()) {
      if (t.name !== "web_fetch" || !t.output_preview) continue;
      let url = "";
      try {
        const args = JSON.parse(t.arguments ?? "{}");
        if (typeof args.url === "string") url = args.url;
      } catch { continue; }
      if (!url) continue;
      const meta = extractLinkMeta(t.output_preview);
      if (meta.title) out[url] = meta.title;
    }
    return out;
  });

  const sessionNarrations = createMemo<NarrationStep[]>(() => {
    const out: NarrationStep[] = [];
    for (const m of messages()) {
      if (m.role !== "agent" || !m.narrations) continue;
      for (const n of m.narrations) out.push(n);
    }
    return out;
  });

  const sessionArtifactEvents = createMemo<ArtifactEvent[]>(() => {
    const out: ArtifactEvent[] = [];
    let agentTurn = 0;
    for (const m of messages()) {
      if (m.role === "decision_draft" && m.decision_draft) {
        out.push({
          id: `decision:${m.decision_draft.deliverable_id}`,
          kind: "decision",
          turn: agentTurn || undefined,
          decision: m.decision_draft,
        });
        continue;
      }
      if (m.role !== "agent") continue;
      agentTurn += 1;
      if (m.artifacts && m.artifacts.length > 0) {
        const scoped = m.artifacts
          .filter(isCanvasArtifact)
          .map((event) => ({ ...event, turn: agentTurn }));
        out.splice(0, out.length, ...mergeArtifactEvents(out, scoped));
        continue;
      }
      for (const n of m.narrations ?? []) {
        out.push({ id: `narration:${n.turn}:${out.length}`, kind: "narration", turn: agentTurn, narration: n });
      }
      for (const s of m.searches ?? []) {
        out.push({ id: searchKey(s), kind: "search", turn: agentTurn, search: s });
      }
      for (const t of m.tool_calls ?? []) {
        if (!isCanvasTool(t)) continue;
        out.splice(0, out.length, ...upsertToolEvent(out, t, agentTurn));
      }
    }
    return out;
  });
  const sessionAgentTurn = createMemo(() => messages().filter((m) => m.role === "agent").length);

  const corpusTools = createMemo<ToolCall[]>(() =>
    sessionTools().filter((t) => t.name === "corpus_search")
  );

  const headTitle = () => messages().length === 0
    ? "NEW SESSION"
    : "CHAT · LIVE";
  const headMeta = () => {
    const turns = messages().filter((m) => m.role === "user").length;
    if (turns === 0) return "0 turns";
    return `${turns} turn${turns > 1 ? "s" : ""} · ${pending() ? "live" : "ready"}`;
  };
  const route = () => `~/sessions/${sessionId()}`;

  return (
    <div class="lk-app" data-screen-label="L.E.E.K · live">
      <Show when={props.onNavigate} fallback={<Rail active="chat" />}>
        {(nav) => <NavRail page="chat" onNavigate={nav()} />}
      </Show>
      <div class="lk-main">
        <TopBar route={route()} />

        <section class="lk-chat">
          <div class="lk-chat-head">
            <SessionMenu
              sessions={sessions()}
              currentId={sessionId()}
              onSelect={switchSession}
              onCreate={createSession}
              onRename={renameSession}
              onDelete={deleteSession}
            />
            <div style={{ display: "flex", gap: "12px", "align-items": "center", "margin-left": "auto" }}>
              <Show when={usage()}>
                <span style={{ color: "var(--ink-3)", "font-size": "11px", "font-family": "var(--font-mono)" }}>
                  in={usage()!.inTokens} · out={usage()!.outTokens}
                </span>
              </Show>
              <button
                onClick={() => setEventsOpen((v) => !v)}
                title="Cmd/Ctrl+E"
                style={{
                  background: eventsOpen() ? "var(--bg-2)" : "transparent",
                  border: "1px solid var(--bg-2)",
                  color: "var(--ink-2)",
                  "border-radius": "6px",
                  padding: "2px 10px",
                  cursor: "pointer",
                  "font-family": "var(--font-mono)",
                  "font-size": "11px",
                }}
              >events</button>
              <span class="lk-chat-head-meta">
                <span style={{
                  "display": "inline-block",
                  width: "8px",
                  height: "8px",
                  "border-radius": "50%",
                  background: connected() ? "#6fb98a" : "#d97070",
                  "margin-right": "6px",
                  "vertical-align": "middle",
                }} />
                {headMeta()}
              </span>
            </div>
          </div>

          <div class="lk-chat-body" ref={(el) => (chatScrollEl = el)}>
            <Show when={messages().length === 0}>
              <div style={{
                color: "var(--ink-3)",
                "font-size": "13px",
                "font-family": "var(--font-mono)",
                padding: "32px 0",
                "text-align": "center",
              }}>
                No messages yet. Type below to start a conversation.
              </div>
            </Show>
            <For each={messages()}>{(m) => (
              <Show
                when={m.role === "agent"}
                fallback={
                  <Show
                    when={m.role === "decision_draft" && m.decision_draft != null}
                    fallback={
                      <Show
                        when={m.role === "compaction_summary"}
                        fallback={<UserMsg time={m.ts}>{m.text}</UserMsg>}
                      >
                        <CompactionSummaryCard time={m.ts} markdown={m.text} />
                      </Show>
                    }
                  >
                    <DecisionDraftCard time={m.ts} draft={m.decision_draft!} />
                  </Show>
                }
              >
                <AgentMsg time={m.ts}>
                  <Show when={m.streaming || m.total_sec != null}>
                    <div style={{
                      "font-family": "var(--font-mono)",
                      "font-size": "10.5px",
                      color: "var(--ink-3)",
                      "margin-bottom": "6px",
                      opacity: m.streaming ? 1 : 0.55,
                    }}>
                      {m.streaming
                        ? `▸ thinking · ${elapsedSec()}s`
                        : `✓ done · ${m.total_sec}s`}
                    </div>
                  </Show>
                  <Show when={(m.searches?.length ?? 0) + (m.tool_calls?.length ?? 0) + (m.narrations?.length ?? 0) > 0}>
                    <div style={{
                      "margin-bottom": "8px",
                      "font-family": "var(--font-mono)",
                      "font-size": "11px",
                      color: "var(--ink-3)",
                      cursor: "pointer",
                    }}>
                      <div
                        onClick={() => scrollToCanvasCard(firstCanvasTarget(m))}
                        title="跳到画布"
                        style={{
                          display: "inline-block",
                          padding: "4px 7px",
                          background: "rgba(255,255,255,0.035)",
                          border: "1px solid var(--line-1)",
                          "border-radius": "5px",
                        }}
                      >
                        {summarizeProcess(m)}
                      </div>
                    </div>
                  </Show>
                  <Show when={m.clarification}>
                    {(clarification) => (
                      <ClarificationRequestCard
                        payload={clarification()}
                        onPick={(text) => void send(text)}
                      />
                    )}
                  </Show>
                  {/* While streaming, show plain text + blinker — markdown
                      renderer would re-parse on every delta, glitching mid-
                      stream (especially for tables / code blocks before they
                      close). Once the message is done, render real markdown
                      so headers / tables / lists / code / links all show up.
                  */}
                  <Show when={!m.clarification || (m.text.trim() && m.text.trim() !== m.clarification.question.trim())}>
                    <Show
                      when={m.streaming}
                      fallback={<SafeMarkdown source={m.text} onWikiOpen={(id) => void openWiki(id)} urlTitles={urlTitles()} />}
                    >
                      <span>{m.text}</span>
                      <span class="lk-stream" />
                    </Show>
                  </Show>
                </AgentMsg>
              </Show>
            )}</For>

            <Show when={compacting()}>
              <CompactingDivider />
            </Show>

            <Show when={providerRetry()}>
              {(retry) => (
                <div style={{
                  color: "#d9a76c",
                  "font-size": "11px",
                  "font-family": "var(--font-mono)",
                  padding: "6px 10px",
                  margin: "8px 0",
                  background: "rgba(217,167,108,0.10)",
                  border: "1px solid rgba(217,167,108,0.20)",
                  "border-radius": "6px",
                }} title={retry().message}>
                  provider 暂时不可用，正在自动恢复 · {retry().delaySec}s 后重试 {retry().retry}/{retry().max}
                </div>
              )}
            </Show>

            <Show when={error()}>
              <div style={{
                color: "#d97070",
                "font-size": "11px",
                "font-family": "var(--font-mono)",
                padding: "6px 10px",
                margin: "8px 0",
                background: "rgba(217,112,112,0.08)",
                "border-radius": "6px",
              }}>
                {error()}
              </div>
            </Show>
          </div>

          <Composer
            placeholder="跟 L.E.E.K 说点什么…"
            onSubmit={send}
            onStop={stop}
            pending={pending() || stopping()}
            compacting={compacting()}
            cancellable={compactionTrigger() !== "auto_pre_turn"}
            commands={slashCommands}
          />
        </section>

        <div
          class="lk-resizer"
          onMouseDown={startResize}
          title="拖动调整 chat 宽度"
        />

        <CanvasArea
          scene={derivedScene()}
          tools={visibleSessionTools()}
          searches={sessionSearches()}
          narrations={sessionNarrations()}
          artifactEvents={sessionArtifactEvents()}
          currentTurn={sessionAgentTurn()}
          corpusTools={corpusTools()}
          plan={agentPlan()}
          onOpenDoc={openWiki}
        />
      </div>

      <EventsPanel
        sessionId={sessionId()}
        open={eventsOpen()}
        onClose={() => setEventsOpen(false)}
        liveTick={liveTick()}
      />

      <Show when={openDoc()}>
        {(doc) => (
          <CorpusDocModal
            doc={doc()}
            loading={docLoading()}
            error={docError()}
            onClose={closeWiki}
            onOpenDoc={(id) => void openWiki(id)}
          />
        )}
      </Show>
    </div>
  );
}

/** Live canvas — tool artifacts stay on the canvas; persistent agent state
 *  (corpus trace + active plan) lives in the right insight sidebar. */
function CanvasArea(props: {
  scene: Scene;
  tools: ToolCall[];
  searches: SearchCall[];
  narrations: NarrationStep[];
  artifactEvents: ArtifactEvent[];
  currentTurn: number;
  corpusTools: ToolCall[];
  plan: AgentPlanView | null;
  onOpenDoc: (id: string, title?: string) => void;
}) {
  const subtitle = () => props.scene === "thinking-shallow"
    ? "reasoning · live"
    : props.scene === "delivered"
    ? "ready · cached"
    : "no thread";

  const hasArtifacts = () =>
    props.artifactEvents.length > 0 || props.tools.length + props.searches.length + props.narrations.length > 0;

  return (
    <div class="lk-canvas">
      <Show when={props.scene !== "idle"}>
        <div class="lk-canvas-head">
          <span class="crumb">
            <b>Live session</b>
            <span class="sep">/</span>
            {subtitle()}
          </span>
        </div>
      </Show>

      <Show
        when={hasArtifacts()}
        fallback={
          <div class="lk-canvas-empty">
            <div class="label">CANVAS · {props.scene === "idle" ? "IDLE" : "QUIET"}</div>
            <div class="sub">Tool outputs and reasoning artifacts materialize here as the agent works.</div>
          </div>
        }
      >
        <ArtifactPanel
          searches={props.searches}
          tools={props.tools}
          narrations={props.narrations}
          events={props.artifactEvents}
          currentTurn={props.currentTurn}
          callbacks={{ onOpenDoc: (id, title) => props.onOpenDoc(id, title) }}
        />
      </Show>

      <InsightSidebar
        scene={props.scene}
        corpusTools={props.corpusTools}
        plan={props.plan}
        onOpenDoc={(id, title) => props.onOpenDoc(id, title)}
      />
    </div>
  );
}
