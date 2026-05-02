// Chat column — messages, agent thinking trace, clarifications, token-stream reveals.
// Ported from prototype/leek-chat.jsx → SolidJS.

import { For, Show, createSignal, type JSX } from "solid-js";
import { Icon } from "./Icon";

/* ---------- token-stream rendering ---------- */

export function StreamText(props: {
  text: string;
  startMs?: number;
  perTok?: number;
  live?: boolean;
  splitOn?: RegExp;
}) {
  const startMs = () => props.startMs ?? 0;
  const perTok = () => props.perTok ?? 22;
  const splitOn = () => props.splitOn ?? /(\s+)/;
  const parts = () => props.text.split(splitOn()).filter((s) => s !== "");
  return (
    <>
      <For each={parts()}>{(p, i) => (
        <span
          class="lk-tok"
          style={{ "animation-delay": (startMs() + i() * perTok()) + "ms" }}
        >{p}</span>
      )}</For>
      <Show when={props.live}><span class="lk-stream" /></Show>
    </>
  );
}

/* ---------- inline references ---------- */

export function NodeRefPill(props: { type: string; id: string; label: string; onClick?: () => void }) {
  return (
    <span class="lk-noderef" data-t={props.type} onClick={props.onClick}>
      <span class="dot" />
      <span style={{ color: "var(--ink-3)" }}>{props.id}</span>
      <span>{props.label}</span>
    </span>
  );
}

export function CorpusCite(props: { tier?: string; path: string }) {
  return (
    <span class="lk-cite" data-tier={props.tier ?? "principles"}>
      <span style={{ color: "var(--ink-3)" }}>↪</span>
      {props.path}
    </span>
  );
}

/* ---------- message primitives ---------- */

function MsgHead(props: { who: "user" | "agent" | "system"; role: string; time: string }) {
  return (
    <div class="lk-msg-head" data-who={props.who}>
      <span class="who">{props.role}</span>
      <span>· {props.time}</span>
    </div>
  );
}

export function UserMsg(props: { time: string; children: JSX.Element }) {
  return (
    <div class="lk-msg" data-who="user">
      <MsgHead who="user" role="YOU" time={props.time} />
      <div class="lk-msg-body user">{props.children}</div>
    </div>
  );
}

export function AgentMsg(props: { time: string; children: JSX.Element }) {
  return (
    <div class="lk-msg" data-who="agent">
      <MsgHead who="agent" role="L.E.E.K" time={props.time} />
      <div class="lk-msg-body">{props.children}</div>
    </div>
  );
}

export function SystemMsg(props: { time: string; children: JSX.Element }) {
  return (
    <div class="lk-msg" data-who="system">
      <MsgHead who="system" role="SYSTEM" time={props.time} />
      <div class="lk-msg-body" style={{ "font-size": "11.5px", color: "var(--ink-2)", "font-family": "var(--font-mono)" }}>
        {props.children}
      </div>
    </div>
  );
}

/* ---------- trace + clarify ---------- */

export interface TraceStep { tag: string; src?: string; text: string; state: string; ms?: number; }

export function TraceBlock(props: { steps: TraceStep[] }) {
  return (
    <div class="lk-trace">
      <For each={props.steps}>{(s) => (
        <div class={"lk-trace-step " + s.state}>
          <span class="tag">{s.tag}</span>
          <Show when={s.src}><span class="src">{s.src}</span></Show>
          <span>{s.text}</span>
          <Show when={s.ms != null}><span class="ms">{s.ms}ms</span></Show>
        </div>
      )}</For>
    </div>
  );
}

export function ClarifyCard(props: { question: string; opts: string[]; picked?: string }) {
  return (
    <div class="lk-clarify">
      <div class="lk-clarify-q">
        <b>L.E.E.K asks ·</b> {props.question}
      </div>
      <div class="lk-clarify-opts">
        <For each={props.opts}>{(o) => (
          <button class={"lk-chip " + (props.picked === o ? "active" : "")}>{o}</button>
        )}</For>
      </div>
    </div>
  );
}

/* ---------- composer ---------- */

export interface SlashCommand {
  name: string;
  hint?: string;
  run: () => void;
}

export function Composer(props: {
  value?: string;
  placeholder?: string;
  focus?: boolean;
  onSubmit?: (text: string) => void;
  onStop?: () => void;
  pending?: boolean;
  commands?: SlashCommand[];
}) {
  const [text, setText] = createSignal(props.value ?? "");
  const [hl, setHl] = createSignal(0);
  const placeholder = () => props.placeholder ?? "Ask the kernel — query a name, draft a thesis, or run a screen…";
  const canSend = () => !props.pending && text().trim().length > 0;
  const trimmed = () => text().trim();
  const slashMode = () => trimmed().startsWith("/");
  const matches = () => {
    if (!slashMode()) return [] as SlashCommand[];
    const q = trimmed().slice(1).toLowerCase();
    return (props.commands ?? []).filter((c) => c.name.toLowerCase().startsWith(q));
  };
  const runMatch = (c: SlashCommand) => { c.run(); setText(""); setHl(0); };
  const submit = () => {
    if (props.pending) return;
    if (slashMode()) {
      const m = matches();
      const pick = m[hl()] ?? m[0];
      if (pick) { runMatch(pick); return; }
      // unknown slash command — fall through to send literal text so the
      // user gets feedback rather than silent failure
    }
    if (!canSend()) return;
    const t = text();
    setText("");
    setHl(0);
    props.onSubmit?.(t);
  };
  // Track IME composition state so Enter doesn't fire while picking
  // pinyin / kana candidates — the textarea sees Enter both as
  // "select candidate" AND as the keydown event we wire submit to,
  // so without this guard a Chinese user typing 'corpus' (which the
  // IME may interpret as pinyin) loses their input on commit.
  let composing = false;
  const onCompositionStart = () => { composing = true; };
  const onCompositionEnd = () => { composing = false; };
  const onKey = (e: KeyboardEvent) => {
    if (matches().length > 0) {
      if (e.key === "ArrowDown") { e.preventDefault(); setHl((h) => Math.min(h + 1, matches().length - 1)); return; }
      if (e.key === "ArrowUp")   { e.preventDefault(); setHl((h) => Math.max(h - 1, 0)); return; }
      if (e.key === "Tab")       { e.preventDefault(); const m = matches()[hl()]; if (m) setText("/" + m.name); return; }
    }
    if (e.key === "Escape" && props.pending) {
      e.preventDefault();
      props.onStop?.();
      return;
    }
    if (e.key === "Escape" && slashMode()) {
      e.preventDefault();
      setText("");
      setHl(0);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !props.pending) {
      if (e.isComposing || composing || (e as KeyboardEvent & { keyCode?: number }).keyCode === 229) return;
      e.preventDefault();
      submit();
    }
  };
  return (
    <div class="lk-composer">
      <Show when={matches().length > 0}>
        <div class="lk-slash-menu">
          <For each={matches()}>{(c, i) => (
            <div
              class={`lk-slash-item${i() === hl() ? " active" : ""}`}
              onMouseEnter={() => setHl(i())}
              onClick={() => runMatch(c)}
            >
              <span class="cmd">/{c.name}</span>
              <Show when={c.hint}><span class="hint">{c.hint}</span></Show>
            </div>
          )}</For>
        </div>
      </Show>
      <div
        class="lk-composer-box"
        style={props.focus ? { "border-color": "rgba(217,119,87,0.45)", "box-shadow": "0 0 0 3px rgba(217,119,87,0.07)" } : {}}
      >
        <textarea
          value={text()}
          placeholder={placeholder()}
          rows={2}
          onInput={(e) => setText(e.currentTarget.value)}
          onKeyDown={onKey}
          onCompositionStart={onCompositionStart}
          onCompositionEnd={onCompositionEnd}
        />
        <div class="lk-composer-row">
          {props.pending ? (
            <button
              class="lk-composer-send"
              style={{ background: "rgba(217,112,112,0.85)" }}
              onClick={() => props.onStop?.()}
              title="Stop generating (Esc)"
            >
              <Icon name="close" class="ic-xs" /> STOP
            </button>
          ) : (
            <button class="lk-composer-send" onClick={submit} disabled={!canSend()}>
              <Icon name="send" class="ic-xs" /> SEND
            </button>
          )}
        </div>
      </div>
      <div class="lk-composer-hint" style={{ "margin-top": "8px" }}>
        <span class="kbd">↵</span> send
        <span class="kbd">⇧↵</span> newline
        <span class="kbd">Esc</span> stop
        <span style={{ "margin-left": "auto" }}>Logic-Enhanced Equity Kernel · v0.4.1</span>
      </div>
    </div>
  );
}
