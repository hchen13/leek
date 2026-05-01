// Live mode — talks to the real Rust gateway via SSE.
// Uses the same chat primitives as the fixture scenes (UserMsg / AgentMsg /
// StreamText / Composer); only the data source is different.

import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { AgentMsg, Composer, StreamText, UserMsg } from "./Chat";

const SESSION_ID = "live";

interface LiveMsg {
  role: "user" | "agent";
  text: string;
  ts: string;
  streaming?: boolean;
}

interface UsageInfo {
  inTokens: number;
  outTokens: number;
}

function fmtTime(d = new Date()) {
  return d.toTimeString().slice(0, 5);
}

export function LiveChat() {
  const [messages, setMessages] = createSignal<LiveMsg[]>([]);
  const [usage, setUsage] = createSignal<UsageInfo | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(false);
  const [connected, setConnected] = createSignal(false);

  let evtSrc: EventSource | undefined;
  let agentBuffer = "";

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
    // so dedupe by ignoring this event.
    evtSrc.addEventListener("user_message", () => {});

    evtSrc.addEventListener("agent_message_start", () => {
      agentBuffer = "";
      setPending(true);
      setMessages((prev) => [
        ...prev,
        { role: "agent", text: "", ts: fmtTime(), streaming: true },
      ]);
    });

    evtSrc.addEventListener("agent_message_delta", (e: MessageEvent) => {
      try {
        const data = JSON.parse(e.data);
        if (typeof data.text === "string") appendDelta(data.text);
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("llm_usage", (e: MessageEvent) => {
      try {
        const u = JSON.parse(e.data);
        setUsage({ inTokens: u.input_tokens ?? 0, outTokens: u.output_tokens ?? 0 });
      } catch {
        // skip malformed
      }
    });

    evtSrc.addEventListener("agent_message_end", () => {
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
    });

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
        <Show when={usage()}>
          <span style={{ color: "var(--ink-3)" }}>
            in={usage()!.inTokens} · out={usage()!.outTokens}
          </span>
        </Show>
      </div>

      <div style={{ flex: 1, overflow: "auto", "padding-right": "8px" }}>
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
              <Show
                when={m.streaming}
                fallback={<span>{m.text}</span>}
              >
                <StreamText text={m.text} live={true} perTok={0} />
              </Show>
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
        disabled={pending()}
      />
    </div>
  );
}
