// L.E.E.K — M1 verification harness.
//
// A minimal chat page for exercising the gateway from a browser: session
// list/create, message list, a composer, a live SSE feed. M1 runs a real
// agent turn — assistant text streams into a pending bubble; tool calls and
// per-turn metrics land in the event log. No later-milestone product
// surfaces — this page exists only to verify the gateway end to end.

import { createSignal, For, onCleanup, Show } from "solid-js";

type Session = {
  id: string;
  title: string | null;
  created_at: string;
  last_active_at: string;
};
type Message = { seq: number; role: string; content: string; created_at: string };
type EventRow = {
  seq: number;
  kind: string;
  payload: Record<string, unknown>;
  created_at: string;
};

// Lifecycle events shown in the right-hand log. `assistant_delta` is handled
// separately — it streams into the pending bubble instead of flooding the log.
const LOG_KINDS = [
  "message_created",
  "tool_call",
  "tool_result",
  "compaction_started",
  "compaction_completed",
  "assistant_done",
  "turn_metrics_recorded",
  "error",
];
const ALL_KINDS = ["assistant_delta", ...LOG_KINDS];

export default function App() {
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [current, setCurrent] = createSignal<string | null>(null);
  const [messages, setMessages] = createSignal<Message[]>([]);
  const [events, setEvents] = createSignal<EventRow[]>([]);
  const [streaming, setStreaming] = createSignal("");
  const [draft, setDraft] = createSignal("");

  let stream: EventSource | undefined;

  const loadSessions = async () => {
    const res = await fetch("/api/v1/sessions");
    const body = await res.json();
    setSessions(body.items ?? []);
  };

  const loadMessages = async (id: string) => {
    const res = await fetch(`/api/v1/sessions/${id}/messages`);
    const body = await res.json();
    setMessages(body.items ?? []);
  };

  const openSession = (id: string) => {
    setCurrent(id);
    setEvents([]);
    setStreaming("");
    void loadMessages(id);
    stream?.close();
    stream = new EventSource(`/stream/sessions/${id}/events`);
    for (const kind of ALL_KINDS) {
      stream.addEventListener(kind, (ev) => {
        const row = JSON.parse((ev as MessageEvent).data) as EventRow;
        if (row.kind === "assistant_delta") {
          setStreaming((prev) => prev + String(row.payload?.text ?? ""));
          return;
        }
        setEvents((prev) => [...prev, row]);
        if (row.kind === "message_created") {
          if (row.payload?.role === "assistant") setStreaming("");
          void loadMessages(id);
        }
      });
    }
  };

  const newSession = async () => {
    const res = await fetch("/api/v1/sessions", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ title: "New session" }),
    });
    const session = (await res.json()) as Session;
    await loadSessions();
    openSession(session.id);
  };

  const send = async () => {
    const id = current();
    const text = draft().trim();
    if (!id || !text) return;
    setDraft("");
    await fetch(`/api/v1/sessions/${id}/messages`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ content: text }),
    });
  };

  onCleanup(() => stream?.close());
  void loadSessions();

  return (
    <div class="app">
      <aside class="sidebar">
        <header>
          <h1>
            L.E.E.K <span>· M1 harness</span>
          </h1>
        </header>
        <button class="new" onClick={() => void newSession()}>
          + New session
        </button>
        <ul class="sessions">
          <For each={sessions()} fallback={<li class="muted">no sessions yet</li>}>
            {(s) => (
              <li
                classList={{ session: true, active: s.id === current() }}
                onClick={() => openSession(s.id)}
              >
                <span class="title">{s.title ?? "(untitled)"}</span>
                <span class="id">{s.id}</span>
              </li>
            )}
          </For>
        </ul>
      </aside>

      <main class="main">
        <Show
          when={current()}
          fallback={<div class="empty">Pick or create a session.</div>}
        >
          <section class="messages">
            <For each={messages()} fallback={<p class="muted">no messages yet</p>}>
              {(m) => (
                <div classList={{ msg: true, [m.role]: true }}>
                  <span class="role">{m.role}</span>
                  <p>{m.content}</p>
                </div>
              )}
            </For>
            <Show when={streaming()}>
              <div class="msg assistant pending">
                <span class="role">assistant · streaming</span>
                <p>{streaming()}</p>
              </div>
            </Show>
          </section>
          <form
            class="composer"
            onSubmit={(e) => {
              e.preventDefault();
              void send();
            }}
          >
            <textarea
              rows={2}
              placeholder="Message — M1 runs a real agent turn (watch the SSE log)"
              value={draft()}
              onInput={(e) => setDraft(e.currentTarget.value)}
            />
            <button type="submit">Send</button>
          </form>
        </Show>
      </main>

      <aside class="events">
        <header>
          <h2>SSE events</h2>
        </header>
        <ul>
          <For each={events()} fallback={<li class="muted">stream idle</li>}>
            {(ev) => (
              <li>
                <span class="kind">{ev.kind}</span>
                <code>{JSON.stringify(ev.payload)}</code>
              </li>
            )}
          </For>
        </ul>
      </aside>
    </div>
  );
}
