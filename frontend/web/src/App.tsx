// L.E.E.K — the research workbench (M1.9.6 / M1.9.7).
//
// A chat-led research workbench: a session list, the Chat column, the
// Canvas execution trace, and a floating Plan / TODO widget. The gateway
// streams the M1.9 event contract over SSE; `store.ts` routes each event to
// its panel by the event's `surface`. This shell owns sessions, the message
// list, the SSE wiring, and the cross-panel "focus a canvas card" action.

import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";

import { Canvas } from "./Canvas";
import { Chat } from "./Chat";
import { CorpusBrain } from "./CorpusBrain";
import { PlanWidget } from "./PlanWidget";
import { Settings } from "./Settings";
import { createWorkbench } from "./store";
import type { EventRow, Message, Session } from "./types";

/** Tab identity for the right column (canvas vs corpus brain) — they
 *  share the same space so each takes the full panel when active. */
type RightTab = "canvas" | "brain";

// SSE event kinds to subscribe to — a subscription detail only. Which panel
// an event lands in is decided by its `surface`, never by this list.
const EVENT_KINDS = [
  "message_created",
  "assistant_delta",
  "note_trace",
  "tool_lifecycle",
  "search_lifecycle",
  "plan_updated",
  "assistant_done",
  "turn_metrics_recorded",
  "compaction_started",
  "compaction_completed",
  // M2.6: emitted just before a turn is soft-stopped for crossing the
  // cost cap — chat surface uses it to render an in-turn warning bar.
  "turn_cost_capped",
  "error",
];

export default function App() {
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [current, setCurrent] = createSignal<string | null>(null);
  const [messages, setMessages] = createSignal<Message[]>([]);
  const [sending, setSending] = createSignal(false);
  const [showFailed, setShowFailed] = createSignal(false);
  const [highlight, setHighlight] = createSignal<string | null>(null);
  const [rightTab, setRightTab] = createSignal<RightTab>("canvas");
  // M2.6: the settings modal is a plain show/hide flag — no router, the
  // panel renders on top of the workbench when this is true.
  const [showSettings, setShowSettings] = createSignal(false);

  const wb = createWorkbench();
  let stream: EventSource | undefined;

  const loadSessions = async () => {
    const res = await fetch("/api/v1/sessions");
    const body = await res.json();
    setSessions(body.items ?? []);
  };

  const loadMessages = async (id: string) => {
    const res = await fetch(`/api/v1/sessions/${id}/messages`);
    const body = await res.json();
    if (current() === id) setMessages(body.items ?? []);
  };

  /** Apply one event — from history replay or the live stream. Persisted
   *  events carry a unique `seq`; `seen` drops any duplicate. */
  const applyOne = (row: EventRow, seen: Set<number>, id: string) => {
    if (typeof row.seq === "number" && row.seq >= 0) {
      if (seen.has(row.seq)) return;
      seen.add(row.seq);
    }
    wb.applyEvent(row);
    if (row.kind === "message_created") {
      void loadMessages(id);
      if (row.payload?.role === "assistant") setSending(false);
    }
  };

  const openSession = async (id: string) => {
    setCurrent(id);
    stream?.close();
    stream = undefined;
    wb.reset();
    setMessages([]);
    setSending(false);
    setShowFailed(false);
    setHighlight(null);

    const seen = new Set<number>();
    await loadMessages(id);

    // Replay durable event history so past turns' canvas is rebuilt.
    try {
      const res = await fetch(`/api/v1/sessions/${id}/events?limit=1000`);
      const body = await res.json();
      for (const row of (body.items ?? []) as EventRow[]) applyOne(row, seen, id);
    } catch {
      // A missing history endpoint is non-fatal — the live stream still runs.
    }
    if (current() !== id) return; // the user switched sessions mid-load

    const es = new EventSource(`/stream/sessions/${id}/events`);
    stream = es;
    for (const kind of EVENT_KINDS) {
      es.addEventListener(kind, (ev) => {
        if (current() !== id) return;
        try {
          applyOne(JSON.parse((ev as MessageEvent).data) as EventRow, seen, id);
        } catch {
          // ignore a malformed frame
        }
      });
    }
  };

  const newSession = async () => {
    const res = await fetch("/api/v1/sessions", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ title: "新会话" }),
    });
    const session = (await res.json()) as Session;
    await loadSessions();
    void openSession(session.id);
  };

  const send = async (text: string) => {
    const id = current();
    if (!id) return;
    setSending(true);
    await fetch(`/api/v1/sessions/${id}/messages`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ content: text }),
    });
    void loadMessages(id);
  };

  /** Scroll to and flash a canvas card — the chat tool summary's click
   *  target. A failed card is hidden by default, so reveal it first. */
  const focusCard = (artifactId: string, isError: boolean) => {
    if (isError) setShowFailed(true);
    setHighlight(artifactId);
    requestAnimationFrame(() => {
      document
        .getElementById(`card-${artifactId}`)
        ?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
    window.setTimeout(() => setHighlight((h) => (h === artifactId ? null : h)), 2200);
  };

  // Badge above the "脑图" tab — count of wikis the agent is touching
  // right now plus those already used in the current turn. Lets the
  // user notice corpus activity without leaving Canvas.
  const brainBadge = createMemo(() => {
    const a = wb.activation();
    if (!a.currentTurn) return 0;
    let n = 0;
    for (const ref of a.byNode.values()) {
      if (ref.liveTurns.has(a.currentTurn) || ref.completedTurns.has(a.currentTurn)) n += 1;
    }
    return n;
  });

  onCleanup(() => stream?.close());
  void loadSessions();

  return (
    <div class="app">
      <aside class="sidebar">
        <header class="sidebar-head">
          <h1>
            L.E.E.K <span>· workbench</span>
          </h1>
          {/* M2.6 ⚙ — opens the settings modal. Plain unicode glyph keeps
              the dependency surface zero; the title attribute is the
              accessible label since the visible glyph is decorative. */}
          <button
            class="settings-gear"
            onClick={() => setShowSettings(true)}
            title="设置"
            aria-label="打开设置"
          >
            ⚙
          </button>
        </header>
        <button class="new" onClick={() => void newSession()}>
          + 新会话
        </button>
        <ul class="sessions">
          <For each={sessions()} fallback={<li class="muted">还没有会话</li>}>
            {(s) => (
              <li
                classList={{ session: true, active: s.id === current() }}
                onClick={() => void openSession(s.id)}
              >
                <span class="title">{s.title ?? "(未命名)"}</span>
                <span class="id">{s.id}</span>
              </li>
            )}
          </For>
        </ul>
      </aside>

      <main class="workbench">
        <Show
          when={current()}
          fallback={<div class="empty-main">选择左侧的会话，或新建一个。</div>}
        >
          <Chat
            messages={messages}
            turns={() => wb.state.turns}
            streaming={() => wb.state.streaming}
            noted={() => wb.state.noted}
            sending={sending}
            send={(t) => void send(t)}
            focusCard={focusCard}
            openSettings={() => setShowSettings(true)}
            sessionId={current}
          />
          <section class="right-rail">
            <nav class="right-tabs">
              <button
                classList={{ "right-tab": true, active: rightTab() === "canvas" }}
                onClick={() => setRightTab("canvas")}
              >
                Canvas
              </button>
              <button
                classList={{ "right-tab": true, active: rightTab() === "brain" }}
                onClick={() => setRightTab("brain")}
              >
                脑图
                <Show when={brainBadge() > 0}>
                  <span class="right-tab-badge">{brainBadge()}</span>
                </Show>
              </button>
            </nav>
            <Show
              when={rightTab() === "canvas"}
              fallback={
                <CorpusBrain
                  activation={wb.activation}
                  sessionId={current}
                />
              }
            >
              <Canvas
                turns={() => wb.state.turns}
                messages={messages}
                showFailed={showFailed}
                setShowFailed={setShowFailed}
                highlight={highlight}
              />
            </Show>
          </section>
          <PlanWidget plan={() => wb.state.plan} />
        </Show>
      </main>

      <Show when={showSettings()}>
        <Settings onClose={() => setShowSettings(false)} />
      </Show>
    </div>
  );
}
