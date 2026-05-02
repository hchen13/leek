// Live mode — talks to the real Rust gateway via SSE.
// Uses the same chat primitives as the fixture scenes (UserMsg / AgentMsg /
// StreamText / Composer); only the data source is different.

import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { AgentMsg, Composer, UserMsg } from "./Chat";
import { EventsPanel } from "./EventsPanel";

const SESSION_ID = "live";

interface SearchCall {
  status: "in_progress" | "completed" | string;
  action: "search" | "open_page" | "find_in_page" | "other" | "unknown" | string;
  detail: string;
}

interface ToolCall {
  call_id: string;
  status: "in_progress" | "completed" | "error" | string;
  name: string;
  arguments?: string;
  output_preview?: string;
  output_bytes?: number;
}

interface LiveMsg {
  role: "user" | "agent";
  text: string;
  ts: string;
  streaming?: boolean;
  searches?: SearchCall[];
  tool_calls?: ToolCall[];
}

function summarizeSearch(s: SearchCall): string {
  const verb = s.status === "completed" ? "✓" : "▸";
  switch (s.action) {
    case "search":
      return `${verb} ${s.status === "completed" ? "Searched" : "Searching"}: ${s.detail || "…"}`;
    case "open_page": {
      let host = s.detail;
      try { host = new URL(s.detail).hostname; } catch { /* keep raw */ }
      return `${verb} ${s.status === "completed" ? "Opened" : "Opening"}: ${host}`;
    }
    case "find_in_page":
      return `${verb} ${s.status === "completed" ? "Searched in page" : "Searching in page"}: ${s.detail}`;
    default:
      return `${verb} ${s.action}`;
  }
}

function summarizeTool(t: ToolCall): string {
  const verb = t.status === "completed" ? "✓" : t.status === "error" ? "✗" : "▸";
  // Try to extract a useful detail from arguments — for web_fetch that's the URL.
  let detail = "";
  if (t.arguments) {
    try {
      const args = JSON.parse(t.arguments);
      if (typeof args.url === "string") {
        try { detail = new URL(args.url).hostname; } catch { detail = args.url; }
      }
    } catch { /* show name only */ }
  }
  const head = t.status === "completed"
    ? `${verb} ${t.name}`
    : t.status === "error"
    ? `${verb} ${t.name} failed`
    : `${verb} ${t.name}`;
  return detail ? `${head}: ${detail}` : head;
}

interface UsageInfo {
  inTokens: number;
  outTokens: number;
}

function fmtTime(d = new Date()) {
  return d.toTimeString().slice(0, 5);
}

interface LiveTick {
  seq: number;
  kind: string;
  payload: unknown;
  ts: string;
}

export function LiveChat() {
  const [messages, setMessages] = createSignal<LiveMsg[]>([]);
  const [usage, setUsage] = createSignal<UsageInfo | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(false);
  const [connected, setConnected] = createSignal(false);
  const [eventsOpen, setEventsOpen] = createSignal(false);
  const [liveTick, setLiveTick] = createSignal<LiveTick | null>(null);

  let evtSrc: EventSource | undefined;
  let agentBuffer = "";
  let chatScrollEl: HTMLDivElement | undefined;

  function emitTick(e: MessageEvent, kind: string, payload: unknown) {
    // The SSE `id` field carries the backend-assigned vault.events.seq —
    // EventsPanel dedupes against that so live ticks merge cleanly with
    // history reloads.
    const seq = parseInt(e.lastEventId, 10);
    if (!Number.isFinite(seq)) return;
    setLiveTick({ seq, kind, payload, ts: new Date().toISOString() });
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

  onMount(async () => {
    // 1. Load history (refresh-survival)
    try {
      const r = await fetch(`/api/v1/sessions/${SESSION_ID}/messages?limit=200`);
      if (r.ok) {
        const json = await r.json();
        const hist: LiveMsg[] = (json.items ?? []).map((m: any) => {
          let text = "";
          try {
            text = JSON.parse(m.content_json).text ?? "";
          } catch {
            // ignore
          }
          return {
            role: m.role === "agent" ? "agent" : "user",
            text,
            ts: typeof m.created_at === "string" ? m.created_at.slice(11, 16) : fmtTime(),
          };
        });
        setMessages(hist);
      }
    } catch {
      // history is optional
    }

    // 2. Subscribe to live event stream
    evtSrc = new EventSource(`/stream/sessions/${SESSION_ID}/events`);

    evtSrc.addEventListener("open", () => setConnected(true));
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
      setMessages((prev) => [
        ...prev,
        { role: "agent", text: "", ts: fmtTime(), streaming: true, searches: [], tool_calls: [] },
      ]);
      try { emitTick(e, "agent_message_start", JSON.parse(e.data)); } catch { /* skip */ }
    });

    evtSrc.addEventListener("tool_call", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        emitTick(e, "tool_call", data);
        const call: ToolCall = {
          call_id: data.call_id,
          status: data.status ?? "in_progress",
          name: data.name ?? "",
          arguments: data.arguments,
          output_preview: data.output_preview,
          output_bytes: data.output_bytes,
        };
        setMessages((prev) => {
          const out = [...prev];
          const last = out[out.length - 1];
          if (!last || last.role !== "agent" || !last.streaming) return prev;
          const tool_calls = [...(last.tool_calls ?? [])];
          // Match completed/error to the in-progress chip with the same call_id.
          if (call.status !== "in_progress") {
            const idx = tool_calls.findIndex((t) => t.call_id === call.call_id);
            if (idx >= 0) {
              // Preserve original arguments since server omits them on completion.
              tool_calls[idx] = { ...tool_calls[idx], ...call, arguments: tool_calls[idx].arguments ?? call.arguments };
            } else {
              tool_calls.push(call);
            }
          } else {
            tool_calls.push(call);
          }
          out[out.length - 1] = { ...last, tool_calls };
          return out;
        });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("web_search_call", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        emitTick(e, "web_search_call", data);
        const call: SearchCall = {
          status: data.status ?? "in_progress",
          action: data.action ?? "unknown",
          detail: data.detail ?? "",
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
          out[out.length - 1] = { ...last, searches };
          return out;
        });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("agent_message_delta", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        if (typeof data.text === "string") appendDelta(data.text);
        emitTick(e, "agent_message_delta", data);
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("llm_usage", (e: MessageEvent) => {
      try {
        const u = JSON.parse(e.data);
        setUsage({ inTokens: u.input_tokens ?? 0, outTokens: u.output_tokens ?? 0 });
        emitTick(e, "llm_usage", u);
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("agent_message_end", (e: MessageEvent) => {
      setPending(false);
      setError(null);
      setMessages((prev) => {
        const out = [...prev];
        const last = out[out.length - 1];
        if (last && last.role === "agent") {
          out[out.length - 1] = { ...last, streaming: false };
        }
        return out;
      });
      try { emitTick(e, "agent_message_end", JSON.parse(e.data)); } catch { /* skip */ }
    });

    // task_created / task_delivered / clarification_requested are emitted by
    // the routing layer for backend audit. P1 UI doesn't surface them per
    // memory:feedback_codex_claude_code_baseline — the chat flow is the
    // user-facing source of truth.

    evtSrc.addEventListener("error", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        setError(data.message ?? "agent error");
      } catch {
        setError("agent error");
      }
      setPending(false);
    });
  });

  onCleanup(() => evtSrc?.close());

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
    // optimistic user message
    setMessages((prev) => [...prev, { role: "user", text, ts: fmtTime() }]);
    try {
      const r = await fetch(`/api/v1/sessions/${SESSION_ID}/messages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ content: { type: "text", text } }),
      });
      if (!r.ok) {
        const body = await r.text();
        setError(`POST ${r.status}: ${body.slice(0, 200)}`);
      }
    } catch (e: any) {
      setError(e?.message ?? "network error");
    }
  }

  async function stop() {
    try {
      await fetch(`/api/v1/sessions/${SESSION_ID}/abort`, { method: "POST" });
    } catch (e: any) {
      setError(e?.message ?? "abort failed");
    }
  }

  return (
    <div style={{
      width: "min(880px, 96vw)",
      height: "min(820px, 92vh)",
      display: "flex",
      "flex-direction": "column",
      gap: "16px",
      padding: "24px",
      background: "var(--bg-1)",
      "border-radius": "10px",
      border: "1px solid var(--bg-2)",
    }}>
      <div style={{
        display: "flex",
        "align-items": "center",
        "justify-content": "space-between",
        "font-family": "var(--font-mono)",
        "font-size": "11px",
      }}>
        <span style={{ color: "var(--ink-2)" }}>
          <span style={{
            "display": "inline-block",
            width: "8px",
            height: "8px",
            "border-radius": "50%",
            background: connected() ? "#6fb98a" : "#d97070",
            "margin-right": "6px",
            "vertical-align": "middle",
          }} />
          LIVE · session={SESSION_ID}
        </span>
        <div style={{ display: "flex", gap: "12px", "align-items": "center" }}>
          <Show when={usage()}>
            <span style={{ color: "var(--ink-3)" }}>
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
        </div>
      </div>

      <div
        ref={(el) => (chatScrollEl = el)}
        style={{ flex: 1, overflow: "auto", "padding-right": "8px" }}
      >
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
            fallback={<UserMsg time={m.ts}>{m.text}</UserMsg>}
          >
            <AgentMsg time={m.ts}>
              <Show when={(m.searches?.length ?? 0) + (m.tool_calls?.length ?? 0) > 0}>
                <div style={{
                  display: "flex",
                  "flex-direction": "column",
                  gap: "2px",
                  "margin-bottom": "8px",
                  "font-family": "var(--font-mono)",
                  "font-size": "11px",
                  color: "var(--ink-3)",
                }}>
                  <For each={m.searches!}>{(s) => (
                    <div style={{
                      opacity: s.status === "completed" ? 1 : 0.7,
                    }}>
                      {summarizeSearch(s)}
                    </div>
                  )}</For>
                  <For each={m.tool_calls!}>{(t) => (
                    <div style={{
                      opacity: t.status === "completed" ? 1 : t.status === "error" ? 1 : 0.7,
                      color: t.status === "error" ? "#d97070" : "var(--ink-3)",
                    }}>
                      {summarizeTool(t)}
                    </div>
                  )}</For>
                </div>
              </Show>
              {/* Plain text + blinker for streaming. We deliberately don't use
                  StreamText here: its lk-tok fade-in animation re-triggers on
                  every delta (esp. CJK where /\s+/ split produces one token),
                  causing visual glitches. StreamText is reserved for fixture
                  scenes where text is pre-rendered. */}
              <span>{m.text}</span>
              <Show when={m.streaming}><span class="lk-stream" /></Show>
            </AgentMsg>
          </Show>
        )}</For>
      </div>

      <Show when={error()}>
        <div style={{
          color: "#d97070",
          "font-size": "11px",
          "font-family": "var(--font-mono)",
          padding: "6px 10px",
          background: "rgba(217,112,112,0.08)",
          "border-radius": "6px",
        }}>
          ⚠ {error()}
        </div>
      </Show>

      <Composer
        placeholder="跟 L.E.E.K 说点什么…"
        onSubmit={send}
        onStop={stop}
        pending={pending()}
      />

      <EventsPanel
        sessionId={SESSION_ID}
        open={eventsOpen()}
        onClose={() => setEventsOpen(false)}
        liveTick={liveTick()}
      />
    </div>
  );
}
