// App — top-level shell (M4.2.1).
//
// 3-column layout (DESIGN.md §4.3): Rail · Chat · Canvas · Sidebar.
// The rail's bottom slot switches the main area between "chat workbench"
// and "settings page" — both share the rail but only the chat workbench
// shows the right sidebar.
//
// SSE wiring + session list management stay here (same pattern as the
// pre-M4.2 App): the store reduces events into reactive state, the
// columns are pure readers. The store API (createWorkbench) is unchanged.

import { createMemo, createSignal, onCleanup, Show } from "solid-js";

import { CanvasColumn } from "./components/CanvasColumn";
import { ChatColumn } from "./components/ChatColumn";
import { CorpusBrain } from "./CorpusBrain";
import { Rail, type RailTab } from "./components/Rail";
import { SessionDrawer } from "./components/SessionDrawer";
import { SettingsPage } from "./components/SettingsPage";
import { Sidebar } from "./components/Sidebar";
import { createWorkbench } from "./store";
import type { EventRow, Message, Session } from "./types";

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
  "turn_cost_capped",
  "subagent_lifecycle",
  "codex_duplicate_url_warning",
  "provider_retry_attempt",
  "error",
];

export default function App() {
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [sessionsLoading, setSessionsLoading] = createSignal(true);
  const [current, setCurrent] = createSignal<string | null>(null);
  const [messages, setMessages] = createSignal<Message[]>([]);
  const [sending, setSending] = createSignal(false);
  const [showFailed, setShowFailed] = createSignal(false);
  const [highlight, setHighlight] = createSignal<string | null>(null);
  const [railTab, setRailTab] = createSignal<RailTab>("chat");
  const [drawerOpen, setDrawerOpen] = createSignal(false);
  const [corpusFullscreen, setCorpusFullscreen] = createSignal(false);

  const wb = createWorkbench();
  let stream: EventSource | undefined;

  const loadSessions = async () => {
    setSessionsLoading(true);
    try {
      const res = await fetch("/api/v1/sessions");
      const body = await res.json();
      setSessions(body.items ?? []);
    } catch {
      // Empty list is the visible result.
    } finally {
      setSessionsLoading(false);
    }
  };

  const loadMessages = async (id: string) => {
    const res = await fetch(`/api/v1/sessions/${id}/messages`);
    const body = await res.json();
    if (current() === id) setMessages(body.items ?? []);
  };

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
    setRailTab("chat");
    stream?.close();
    stream = undefined;
    wb.reset();
    setMessages([]);
    setSending(false);
    setShowFailed(false);
    setHighlight(null);

    const seen = new Set<number>();
    await loadMessages(id);

    try {
      const res = await fetch(`/api/v1/sessions/${id}/events?limit=1000`);
      const body = await res.json();
      for (const row of (body.items ?? []) as EventRow[]) applyOne(row, seen, id);
    } catch {
      // history endpoint optional
    }
    if (current() !== id) return;

    const es = new EventSource(`/stream/sessions/${id}/events`);
    stream = es;
    for (const kind of EVENT_KINDS) {
      es.addEventListener(kind, (ev) => {
        if (current() !== id) return;
        try {
          applyOne(JSON.parse((ev as MessageEvent).data) as EventRow, seen, id);
        } catch {
          // ignore malformed frame
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

  // Title for the chat header — pull from the session list, fall back to
  // a friendly placeholder.
  const sessionTitle = createMemo(() => {
    const id = current();
    if (!id) return "L.E.E.K · 投研工作台";
    const s = sessions().find((x) => x.id === id);
    return s?.title ?? "(未命名)";
  });

  onCleanup(() => stream?.close());
  void loadSessions();

  const onRailTab = (t: RailTab) => {
    setRailTab(t);
    if (t === "chat") setDrawerOpen(false);
  };

  return (
    <div class="lk-app">
      <Rail active={railTab} onTab={onRailTab} />
      <main class="lk-main">
        <Show
          when={railTab() === "chat"}
          fallback={<SettingsPage onBack={() => setRailTab("chat")} />}
        >
          <Show
            when={current()}
            fallback={
              <div class="lk-empty lk-main-empty">
                <span class="lk-empty-title">选择一个会话开始</span>
                <span class="lk-empty-hint">
                  点击左侧聊天图标边的历史按钮, 或新建一个会话。
                </span>
                <button
                  class="lk-btn lk-btn--primary"
                  type="button"
                  onClick={() => void newSession()}
                >
                  新建会话
                </button>
              </div>
            }
          >
            <ChatColumn
              messages={messages}
              turns={() => wb.state.turns}
              streaming={() => wb.state.streaming}
              noted={() => wb.state.noted}
              sending={sending}
              send={(t) => void send(t)}
              focusCard={focusCard}
              openSettings={() => setRailTab("settings")}
              sessionId={current}
              sessionTitle={sessionTitle}
              onOpenDrawer={() => setDrawerOpen(true)}
              onNewSession={() => void newSession()}
            />
            <CanvasColumn
              turns={() => wb.state.turns}
              messages={messages}
              showFailed={showFailed}
              setShowFailed={setShowFailed}
              highlight={highlight}
              sessionTitle={sessionTitle}
            />
            <Sidebar
              activation={wb.activation}
              plan={() => wb.state.plan}
              onFullscreenCorpus={() => setCorpusFullscreen(true)}
            />
          </Show>
        </Show>
      </main>

      <SessionDrawer
        open={drawerOpen}
        sessions={sessions}
        loading={sessionsLoading}
        currentId={current}
        onClose={() => setDrawerOpen(false)}
        onPick={(id) => void openSession(id)}
        onNew={() => void newSession()}
      />

      {/* Corpus fullscreen overlay — uses the original CorpusBrain. */}
      <Show when={corpusFullscreen()}>
        <div class="lk-corpus-fullscreen" role="dialog" aria-label="corpus 全图">
          <button
            class="lk-icon-btn lk-corpus-fullscreen-close"
            type="button"
            onClick={() => setCorpusFullscreen(false)}
            title="关闭"
            aria-label="关闭 corpus 全图"
          >
            ✕
          </button>
          <CorpusBrain activation={wb.activation} sessionId={current} />
        </div>
      </Show>
    </div>
  );
}
