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
    <div class="lk-msg">
      <MsgHead who="user" role="YOU" time={props.time} />
      <div class="lk-msg-body user">{props.children}</div>
    </div>
  );
}

export function AgentMsg(props: { time: string; children: JSX.Element }) {
  return (
    <div class="lk-msg">
      <MsgHead who="agent" role="L.E.E.K" time={props.time} />
      <div class="lk-msg-body">{props.children}</div>
    </div>
  );
}

export function SystemMsg(props: { time: string; children: JSX.Element }) {
  return (
    <div class="lk-msg">
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

export function Composer(props: {
  value?: string;
  placeholder?: string;
  focus?: boolean;
  onSubmit?: (text: string) => void;
  disabled?: boolean;
}) {
  const [text, setText] = createSignal(props.value ?? "");
  const placeholder = () => props.placeholder ?? "Ask the kernel — query a name, draft a thesis, or run a screen…";
  const canSend = () => !props.disabled && text().trim().length > 0;
  const submit = () => {
    if (!canSend()) return;
    const t = text();
    setText("");
    props.onSubmit?.(t);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };
  return (
    <div class="lk-composer">
      <div
        class="lk-composer-box"
        style={props.focus ? { "border-color": "rgba(217,119,87,0.45)", "box-shadow": "0 0 0 3px rgba(217,119,87,0.07)" } : {}}
      >
        <textarea
          value={text()}
          placeholder={placeholder()}
          rows={2}
          disabled={props.disabled}
          onInput={(e) => setText(e.currentTarget.value)}
          onKeyDown={onKey}
        />
        <div class="lk-composer-row">
          <button class="lk-composer-tool" title="Attach"><Icon name="paperclip" class="ic-sm" /></button>
          <button class="lk-composer-tool" title="Reference"><Icon name="pin" class="ic-sm" /></button>
          <button class="lk-composer-tool" title="Voice"><Icon name="mic" class="ic-sm" /></button>
          <span class="lk-chip-mini">corpus · 14.2K docs</span>
          <span class="lk-chip-mini">tools · 9</span>
          <button class="lk-composer-send" onClick={submit} disabled={!canSend()}>
            <Icon name="send" class="ic-xs" /> SEND
          </button>
        </div>
      </div>
      <div class="lk-composer-hint" style={{ "margin-top": "8px" }}>
        <span class="kbd">↵</span> send
        <span class="kbd">⇧↵</span> newline
        <span style={{ "margin-left": "auto" }}>Logic-Enhanced Equity Kernel · v0.4.1</span>
      </div>
    </div>
  );
}
