// Events Timeline — drawer-style panel that lists every vault.events row
// for the active session. Opens with Cmd/Ctrl+E. Re-fetches on open and
// stays in sync via the live SSE stream (LiveChat broadcasts via callback).

import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";

interface EventRow {
  seq: number;
  task_id: string | null;
  kind: string;
  payload_json: string;
  source: string | null;
  ts: string;
}

const KIND_FILTERS: { id: string; label: string; predicate: (k: string) => boolean }[] = [
  { id: "all",     label: "all",     predicate: () => true },
  { id: "agent",   label: "agent",   predicate: (k) => k.startsWith("agent_message_") },
  { id: "tool",    label: "tool",    predicate: (k) => k === "tool_call" || k === "web_search_call" },
  { id: "user",    label: "user",    predicate: (k) => k === "user_message" },
  { id: "task",    label: "task",    predicate: (k) => k.startsWith("task_") || k === "deliverable_ready" || k === "clarification_requested" },
  { id: "usage",   label: "usage",   predicate: (k) => k === "llm_usage" },
  { id: "error",   label: "error",   predicate: (k) => k === "error" },
];

function fmtTs(s: string): string {
  // RFC3339 → HH:MM:SS
  const i = s.indexOf("T");
  return i >= 0 ? s.slice(i + 1, i + 9) : s;
}

function summarize(kind: string, payload: unknown): string {
  if (typeof payload !== "object" || payload === null) return "";
  const p = payload as Record<string, unknown>;
  switch (kind) {
    case "user_message":
    case "agent_message_delta": {
      const t = (p.text as string) ?? "";
      return t.slice(0, 100);
    }
    case "agent_message_start":
      return p.task_id ? `task=${p.task_id}` : "";
    case "agent_message_end":
      return `stop=${p.stop_reason ?? ""} seq=${p.message_seq ?? ""}`;
    case "web_search_call":
      return `${p.action ?? ""} ${p.status ?? ""} ${p.detail ?? ""}`.trim();
    case "tool_call":
      return `${p.name ?? ""} ${p.status ?? ""} ${p.output_bytes ? `(${p.output_bytes}B)` : ""}`.trim();
    case "llm_usage":
      return `in=${p.input_tokens ?? 0} out=${p.output_tokens ?? 0} cache=${p.cache_read_tokens ?? 0}`;
    case "task_created":
      return `${p.title ?? p.task_id ?? ""}`;
    case "task_started":
    case "task_delivered":
      return `${p.task_id ?? ""}`;
    case "deliverable_ready":
      return `kind=${p.kind ?? ""} id=${p.deliverable_id ?? ""}`;
    case "clarification_requested":
      return (p.question as string)?.slice(0, 100) ?? "";
    case "error":
      return ((p.message as string) ?? "").slice(0, 120);
    default:
      return "";
  }
}

function kindColor(kind: string): string {
  if (kind.startsWith("agent_message_")) return "#9eb78f";
  if (kind === "tool_call" || kind === "web_search_call") return "#d9a76c";
  if (kind === "user_message") return "#a89e8c";
  if (kind.startsWith("task_") || kind === "deliverable_ready") return "#c990d0";
  if (kind === "llm_usage") return "#7d99b3";
  if (kind === "error") return "#d97070";
  return "#a89e8c";
}

export function EventsPanel(props: {
  sessionId: string;
  open: boolean;
  onClose: () => void;
  /** When the live stream emits a new event, parent calls this; we append. */
  liveTick?: { seq: number; kind: string; payload: unknown; ts: string } | null;
}) {
  const [events, setEvents] = createSignal<EventRow[]>([]);
  const [filter, setFilter] = createSignal<string>("all");
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [expanded, setExpanded] = createSignal<Record<number, boolean>>({});

  async function loadAll() {
    setLoading(true);
    setError(null);
    try {
      const r = await fetch(`/api/v1/sessions/${props.sessionId}/events?limit=2000`);
      if (!r.ok) {
        setError(`HTTP ${r.status}`);
        return;
      }
      const j = await r.json();
      setEvents(j.items ?? []);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "network error");
    } finally {
      setLoading(false);
    }
  }

  // Load on first open + on every (re)open. Also listen for live ticks while open.
  createEffect(() => {
    if (props.open) void loadAll();
  });

  createEffect(() => {
    const tick = props.liveTick;
    if (!tick || !props.open) return;
    setEvents((prev) => {
      if (prev.some((e) => e.seq === tick.seq)) return prev;
      return [
        ...prev,
        {
          seq: tick.seq,
          task_id: null,
          kind: tick.kind,
          payload_json: JSON.stringify(tick.payload ?? {}),
          source: null,
          ts: tick.ts,
        },
      ];
    });
  });

  const visible = createMemo(() => {
    const f = KIND_FILTERS.find((x) => x.id === filter()) ?? KIND_FILTERS[0];
    return events().filter((e) => f.predicate(e.kind));
  });

  // Latest at top — easier to spot the just-emitted event during dogfooding.
  const ordered = createMemo(() => visible().slice().reverse());

  function toggle(seq: number) {
    setExpanded((prev) => ({ ...prev, [seq]: !prev[seq] }));
  }

  // Close on Esc, also handled at parent for Cmd+E toggle.
  createEffect(() => {
    if (!props.open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
    };
    document.addEventListener("keydown", handler);
    onCleanup(() => document.removeEventListener("keydown", handler));
  });

  return (
    <Show when={props.open}>
      <div
        onClick={props.onClose}
        style={{
          position: "fixed",
          inset: 0,
          background: "rgba(8, 6, 4, 0.45)",
          "z-index": 900,
        }}
      />
      <aside
        onClick={(e) => e.stopPropagation()}
        style={{
          position: "fixed",
          top: 0,
          right: 0,
          bottom: 0,
          width: "min(620px, 92vw)",
          background: "var(--bg-1)",
          "border-left": "1px solid var(--bg-2)",
          padding: "20px 22px",
          display: "flex",
          "flex-direction": "column",
          gap: "12px",
          "z-index": 901,
          color: "var(--ink-1)",
          "font-family": "var(--font-mono)",
        }}
      >
        <div style={{
          display: "flex",
          "align-items": "baseline",
          "justify-content": "space-between",
        }}>
          <div>
            <div style={{ "font-size": "13px", "font-weight": 600, "font-family": "var(--font-sans)" }}>
              Events Timeline
            </div>
            <div style={{ "font-size": "10px", color: "var(--ink-3)", "margin-top": "2px" }}>
              session={props.sessionId} · {events().length} rows
            </div>
          </div>
          <button
            onClick={props.onClose}
            style={{
              background: "transparent",
              border: "1px solid var(--bg-2)",
              color: "var(--ink-2)",
              "border-radius": "6px",
              padding: "3px 10px",
              cursor: "pointer",
              "font-size": "10px",
            }}
          >Esc</button>
        </div>
        <div style={{ display: "flex", "flex-wrap": "wrap", gap: "4px" }}>
          <For each={KIND_FILTERS}>{(f) => (
            <button
              onClick={() => setFilter(f.id)}
              style={{
                background: filter() === f.id ? "var(--bg-2)" : "transparent",
                border: "1px solid var(--bg-2)",
                color: filter() === f.id ? "var(--ink-1)" : "var(--ink-3)",
                "border-radius": "10px",
                padding: "2px 10px",
                cursor: "pointer",
                "font-size": "10px",
              }}
            >{f.label}</button>
          )}</For>
        </div>
        <Show when={loading()}>
          <div style={{ color: "var(--ink-3)", "font-size": "11px" }}>loading…</div>
        </Show>
        <Show when={error()}>
          <div style={{ color: "#d97070", "font-size": "11px" }}>error: {error()}</div>
        </Show>
        <div style={{
          flex: 1,
          overflow: "auto",
          "padding-right": "6px",
          "font-size": "11px",
        }}>
          <For each={ordered()}>{(ev) => {
            const payload = (() => {
              try { return JSON.parse(ev.payload_json); }
              catch { return {}; }
            })();
            const sm = summarize(ev.kind, payload);
            const exp = !!expanded()[ev.seq];
            return (
              <div
                onClick={() => toggle(ev.seq)}
                style={{
                  padding: "6px 8px",
                  "border-bottom": "1px solid rgba(255,255,255,0.04)",
                  cursor: "pointer",
                }}
              >
                <div style={{
                  display: "flex",
                  gap: "8px",
                  "align-items": "baseline",
                }}>
                  <span style={{ color: "var(--ink-3)", "min-width": "32px" }}>#{ev.seq}</span>
                  <span style={{ color: "var(--ink-3)", "min-width": "60px" }}>{fmtTs(ev.ts)}</span>
                  <span style={{
                    color: kindColor(ev.kind),
                    "min-width": "180px",
                  }}>{ev.kind}</span>
                  <span style={{
                    color: "var(--ink-2)",
                    flex: 1,
                    overflow: "hidden",
                    "text-overflow": "ellipsis",
                    "white-space": "nowrap",
                  }}>{sm}</span>
                </div>
                <Show when={exp}>
                  <pre style={{
                    "margin-top": "6px",
                    "margin-left": "32px",
                    "background": "rgba(0,0,0,0.18)",
                    "padding": "6px 8px",
                    "border-radius": "4px",
                    "font-size": "10.5px",
                    "white-space": "pre-wrap",
                    "color": "var(--ink-2)",
                    "max-height": "300px",
                    "overflow": "auto",
                  }}>{JSON.stringify(payload, null, 2)}</pre>
                </Show>
              </div>
            );
          }}</For>
          <Show when={!loading() && ordered().length === 0}>
            <div style={{
              color: "var(--ink-3)",
              padding: "32px 0",
              "text-align": "center",
            }}>no events match filter</div>
          </Show>
        </div>
        <div style={{ color: "var(--ink-3)", "font-size": "10px" }}>
          Cmd/Ctrl+E to toggle · click row to expand JSON
        </div>
      </aside>
    </Show>
  );
}
