// L.E.E.K — M0 verification harness.
//
// Deliberately minimal: session list/create, message list, a composer, and a
// raw SSE event log. No later-milestone product surfaces —
// those belong to later milestones. This page exists only to prove the
// gateway's HTTP + SSE plumbing from a browser.

import { createSignal, For, onCleanup, Show } from "solid-js";

type Session = {
  id: string;
  title: string | null;
  created_at: string;
  last_active_at: string;
};
type Message = { seq: number; role: string; content: string; created_at: string };
type EventRow = { seq: number; kind: string; payload: unknown; created_at: string };

export default function App() {
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [current, setCurrent] = createSignal<string | null>(null);
  const [messages, setMessages] = createSignal<Message[]>([]);
  const [events, setEvents] = createSignal<EventRow[]>([]);
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
    void loadMessages(id);
    stream?.close();
    stream = new EventSource(`/stream/sessions/${id}/events`);
    for (const kind of ["message_created", "assistant_delta", "assistant_done"]) {
      stream.addEventListener(kind, (ev) => {
        const row = JSON.parse((ev as MessageEvent).data) as EventRow;
        setEvents((prev) => [...prev, row]);
        if (row.kind === "message_created") void loadMessages(id);
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
          <h1>L.E.E.K <span>· M0 harness</span></h1>
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
              placeholder="Message — M0 replies with Echo:"
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
