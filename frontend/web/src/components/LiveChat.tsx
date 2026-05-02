// Live mode — talks to the real Rust gateway via SSE.
// Uses the same chat primitives as the fixture scenes (UserMsg / AgentMsg /
// StreamText / Composer); only the data source is different.

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { AgentMsg, Composer, UserMsg } from "./Chat";
import { EventsPanel } from "./EventsPanel";
import { BrainWidget } from "./BrainWidget";
import { Rail, TopBar } from "./Workbench";
import { SafeMarkdown } from "./SafeMarkdown";
import { ArtifactPanel } from "./ArtifactCards";
import { SessionMenu, type SessionRow } from "./SessionMenu";
import type { Scene } from "../scenes";

const DEFAULT_SESSION_ID = "live";

function readSessionFromHash(): string {
  const h = window.location.hash.replace(/^#/, "");
  if (h.startsWith("s/")) return h.slice(2);
  return DEFAULT_SESSION_ID;
}
function writeSessionToHash(id: string) {
  if (id === DEFAULT_SESSION_ID) {
    if (window.location.hash) history.replaceState(null, "", window.location.pathname);
  } else {
    window.location.hash = `s/${id}`;
  }
}

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
  /** Final elapsed seconds for the agent reply, frozen at message_end. */
  total_sec?: number;
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
  const [sessionId, setSessionId] = createSignal<string>(readSessionFromHash());
  const [sessions, setSessions] = createSignal<SessionRow[]>([]);
  const [messages, setMessages] = createSignal<LiveMsg[]>([]);
  const [usage, setUsage] = createSignal<UsageInfo | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(false);
  const [connected, setConnected] = createSignal(false);
  const [eventsOpen, setEventsOpen] = createSignal(false);
  const [liveTick, setLiveTick] = createSignal<LiveTick | null>(null);
  // Wall-clock seconds since the current agent reply started. Drives the
  // "thinking · 24s" status row above the streaming message.
  const [elapsedSec, setElapsedSec] = createSignal(0);

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
    try {
      await fetch(`/api/v1/sessions/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      });
      await refreshSessions();
    } catch {/* ignore */}
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
    // 1. Load history (refresh-survival)
    setMessages([]);
    setUsage(null);
    setPending(false);
    try {
      const r = await fetch(`/api/v1/sessions/${id}/messages?limit=200`);
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
    evtSrc = new EventSource(`/stream/sessions/${id}/events`);

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
      // Start the elapsed-time counter for the "thinking · Ns" status row.
      agentStartTs = Date.now();
      setElapsedSec(0);
      if (elapsedTimer) clearInterval(elapsedTimer);
      elapsedTimer = window.setInterval(() => {
        setElapsedSec(Math.max(0, Math.floor((Date.now() - agentStartTs) / 1000)));
      }, 1000);
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
  }

  function switchSession(id: string) {
    if (id === sessionId()) return;
    evtSrc?.close();
    evtSrc = undefined;
    setSessionId(id);
    writeSessionToHash(id);
    void connect(id);
  }

  onMount(() => {
    void connect(sessionId());
    void refreshSessions();
    // Pick up forward/back navigation that flips the session hash.
    const onHash = () => {
      const next = readSessionFromHash();
      if (next !== sessionId()) switchSession(next);
    };
    window.addEventListener("hashchange", onHash);
    onCleanup(() => window.removeEventListener("hashchange", onHash));
  });

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
    // optimistic user message
    setMessages((prev) => [...prev, { role: "user", text, ts: fmtTime() }]);
    try {
      const r = await fetch(`/api/v1/sessions/${sessionId()}/messages`, {
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
      await fetch(`/api/v1/sessions/${sessionId()}/abort`, { method: "POST" });
    } catch (e: any) {
      setError(e?.message ?? "abort failed");
    }
  }

  // Derive a Scene from real session state so the Workbench shell (canvas
  // header / brain meta / etc) shows the right form. `idle` until a turn
  // happens; `thinking-shallow` while pending; `delivered` once a reply
  // exists. `clarify` / `deep` are reserved for routing-layer signals
  // (clarification_requested) and active multi-turn task work — wired
  // when those events surface in the UI.
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
    const seen = new Set<string>();
    for (const m of messages()) {
      if (m.role !== "agent" || !m.tool_calls) continue;
      for (const t of m.tool_calls) {
        if (!seen.has(t.call_id)) {
          seen.add(t.call_id);
          out.push(t);
        }
      }
    }
    return out;
  });
  const sessionSearches = createMemo<SearchCall[]>(() => {
    const out: SearchCall[] = [];
    for (const m of messages()) {
      if (m.role !== "agent" || !m.searches) continue;
      for (const s of m.searches) out.push(s);
    }
    return out;
  });

  // Wikilink ids the agent has touched via corpus_search this session.
  // Drives the brain widget's activation halo. We scrape the truncated
  // output_preview because the full output isn't kept client-side; ids
  // that get cut mid-string are simply skipped — better than nothing
  // until we wire structured tool outputs end-to-end.
  const ID_RE = /"id":"((?:wikis|sources)\/[^"]+)"/g;
  const activatedCorpusIds = createMemo<string[]>(() => {
    const ids = new Set<string>();
    for (const t of sessionTools()) {
      if (t.name !== "corpus_search" || !t.output_preview) continue;
      let m: RegExpExecArray | null;
      ID_RE.lastIndex = 0;
      while ((m = ID_RE.exec(t.output_preview)) !== null) {
        ids.add(m[1]);
      }
    }
    return Array.from(ids);
  });

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
      <Rail active="chat" />
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
                fallback={<UserMsg time={m.ts}>{m.text}</UserMsg>}
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
                  {/* While streaming, show plain text + blinker — markdown
                      renderer would re-parse on every delta, glitching mid-
                      stream (especially for tables / code blocks before they
                      close). Once the message is done, render real markdown
                      so headers / tables / lists / code / links all show up.
                  */}
                  <Show when={m.streaming} fallback={<SafeMarkdown source={m.text} />}>
                    <span>{m.text}</span>
                    <span class="lk-stream" />
                  </Show>
                </AgentMsg>
              </Show>
            )}</For>

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
                ⚠ {error()}
              </div>
            </Show>
          </div>

          <Composer
            placeholder="跟 L.E.E.K 说点什么…"
            onSubmit={send}
            onStop={stop}
            pending={pending()}
          />
        </section>

        <CanvasArea
          scene={derivedScene()}
          tools={sessionTools()}
          searches={sessionSearches()}
          activatedIds={activatedCorpusIds()}
        />
      </div>

      <EventsPanel
        sessionId={sessionId()}
        open={eventsOpen()}
        onClose={() => setEventsOpen(false)}
        liveTick={liveTick()}
      />
    </div>
  );
}

/** Live canvas — currently shows the most recent tool call activity as a
 *  list of "artifact" rows on the left, plus the brain on the right. As we
 *  wire richer tool outputs (charts, tables, filings preview) each kind
 *  will render its own Panel-style component. */
function CanvasArea(props: {
  scene: Scene;
  tools: ToolCall[];
  searches: SearchCall[];
  activatedIds: string[];
}) {
  const subtitle = () => props.scene === "thinking-shallow"
    ? "reasoning · live"
    : props.scene === "delivered"
    ? "ready · cached"
    : "no thread";

  const hasArtifacts = () => props.tools.length + props.searches.length > 0;

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
        />
      </Show>

      <BrainWidget scene={props.scene} fireIds={undefined} activatedIds={props.activatedIds} />
    </div>
  );
}

