// StatusPill — the per-turn "agent is doing X" indicator + abort button
// (DESIGN.md §5.3). Sits in the chat composer area while a turn runs and
// disappears once the turn settles.
//
// This is the refactored M3.2 TurnStatusPill, kept with the same semantics
// (urgency rungs, elapsed clock, abort POST) but rebuilt against the
// design-system tokens — `--info` / `--ok` / `--warn` / `--danger` for the
// pill background tint, `--accent` for the spinner glyph, `--motion-pulse`
// for the dot blink. No hex inline.
//
// The two pure helpers `describeActivity` + `urgencyLevel` are re-exported
// so the existing turn_status_pill.test.mjs continues to anchor the
// thresholds (those tests re-implement the helpers; we still export them
// for any future inline test). State coverage:
//   - running + no signal yet  → "正在启动…"
//   - running + tool active    → "正在调用 X"
//   - running + over 1 min idle→ "深度思考中…"
//   - retry in flight          → "重试中 (N/M) …" (overrides activity)

import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import { Icon } from "./Icon";
import type { Turn, TurnActivity } from "../types";

type Props = {
  /** The running turn, or null when nothing is in flight. */
  turn: () => Turn | null;
  /** Session id — used to POST the abort request. */
  sessionId: () => string | null;
};

/** Format an elapsed-ms value into a Chinese time string: "Xs" → "X秒",
 *  longer runs read as "Xm Ys" → "X分YY秒". */
function fmtElapsed(ms: number): string {
  if (ms < 0) ms = 0;
  const totalSec = Math.floor(ms / 1000);
  if (totalSec < 60) return `${totalSec}秒`;
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m}分${s.toString().padStart(2, "0")}秒`;
}

/** Status text the pill reads. Exported for tests. */
export function describeActivity(activity: TurnActivity | null, elapsedMs: number): string {
  if (activity?.kind === "tool") return `正在调用 ${activity.displayName}`;
  if (activity?.kind === "search") return "正在搜索网页（codex 内置）";
  if (activity?.kind === "delta") return "正在写回答";
  if (elapsedMs >= 60_000) return "深度思考中（codex 内部可能多次搜索）";
  if (elapsedMs >= 5_000) return "agent 正在思考…";
  return "正在启动…";
}

/** Pill background rung — 4 rungs not 3 so a fresh retry can flag warn
 *  immediately without waiting for the time-based threshold. Exported
 *  for tests. */
export function urgencyLevel(elapsedSinceSignalMs: number): "calm" | "warm" | "hot" {
  if (elapsedSinceSignalMs >= 180_000) return "hot";
  if (elapsedSinceSignalMs >= 60_000) return "warm";
  return "calm";
}

/** Map a retry's backend kind string into a tooltip for the kind chip
 *  inside the pill. Exhaustive list mirrors backend `FatalReason::kind`. */
function describeRetryKind(kind: string): string {
  if (kind === "codex_http_5xx") return "codex 服务端返回 5xx,正在重试";
  if (kind === "codex_connection_failed") return "无法连接 codex(DNS / 网络抖动),正在重试";
  if (kind === "codex_stream_silent") return "codex 流静默,正在重试";
  return `provider 错误(${kind}),正在重试`;
}

export function StatusPill(props: Props) {
  const [nowMs, setNowMs] = createSignal(Date.now());
  const [aborting, setAborting] = createSignal(false);
  const [lastSignalAtMs, setLastSignalAtMs] = createSignal(Date.now());
  let prevActivityKey: string | null = null;
  let prevTurnId: string | null = null;

  // Re-arm the signal clock when activity flips. Solid's createEffect
  // re-runs on any tracked dependency change, so this is the natural
  // place to do it.
  createEffect(() => {
    const t = props.turn();
    if (!t) {
      prevActivityKey = null;
      prevTurnId = null;
      return;
    }
    if (t.turnId !== prevTurnId) {
      prevTurnId = t.turnId;
      prevActivityKey = null;
      setLastSignalAtMs(Date.now());
    }
    const key = t.activity == null
      ? "_idle"
      : t.activity.kind === "tool"
        ? `tool:${t.activity.displayName}`
        : t.activity.kind;
    if (key !== prevActivityKey) {
      prevActivityKey = key;
      setLastSignalAtMs(Date.now());
    }
  });

  const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
  onCleanup(() => window.clearInterval(timer));

  const iteration = () => {
    const t = props.turn();
    if (!t) return 0;
    let max = 0;
    for (const a of t.artifacts) {
      if (a.iteration > max) max = a.iteration;
    }
    return max;
  };

  const elapsedMs = () => {
    const t = props.turn();
    if (!t?.startedAtMs) return 0;
    return nowMs() - t.startedAtMs;
  };

  const urgency = () => urgencyLevel(nowMs() - lastSignalAtMs());

  const onAbort = async () => {
    const t = props.turn();
    const sid = props.sessionId();
    if (!t || !sid || aborting()) return;
    if (!window.confirm("中止本回合？已收到的文字会保留。")) return;
    setAborting(true);
    try {
      const res = await fetch(
        `/api/v1/sessions/${sid}/turns/${t.turnId}/abort`,
        { method: "POST" },
      );
      if (!res.ok && res.status !== 404) {
        console.warn("abort POST failed", res.status, await res.text());
      }
    } catch (e) {
      console.warn("abort POST threw", e);
    } finally {
      setAborting(false);
    }
  };

  return (
    <Show when={props.turn() && props.turn()!.status === "running"}>
      <div
        classList={{
          "lk-pill": true,
          [`lk-pill--${urgency()}`]: true,
          "lk-pill--retrying": props.turn()!.retry != null,
        }}
        role="status"
        aria-live="polite"
      >
        <div class="lk-pill-row">
          <span class="lk-pill-spinner" aria-hidden="true">
            <Icon name="dot" size={10} />
          </span>
          <span class="lk-pill-text">
            <span class="lk-pill-iter lk-num">回合进行中 · iter {Math.max(1, iteration())}</span>
            <span class="lk-pill-sep">·</span>
            <span class="lk-pill-elapsed lk-num">已用 {fmtElapsed(elapsedMs())}</span>
            <span class="lk-pill-sep">·</span>
            <Show
              when={props.turn()!.retry != null}
              fallback={
                <span class="lk-pill-activity">
                  {describeActivity(props.turn()!.activity, elapsedMs())}
                </span>
              }
            >
              <span class="lk-pill-retry" title={describeRetryKind(props.turn()!.retry!.kind)}>
                重试中 ({props.turn()!.retry!.attempt}/{props.turn()!.retry!.maxAttempts})…
                <Show when={props.turn()!.retry!.backoffMs > 0}>
                  <span class="lk-pill-retry-backoff">
                    {" "}等 {Math.round(props.turn()!.retry!.backoffMs / 1000)} 秒
                  </span>
                </Show>
              </span>
            </Show>
          </span>
          <button
            class="lk-pill-abort"
            onClick={() => void onAbort()}
            disabled={aborting()}
            title="向 agent 发起中止信号"
            type="button"
          >
            <Icon name="x" size={14} />
            <span>{aborting() ? "停止中…" : "强制停止"}</span>
          </button>
        </div>
        <Show when={urgency() === "hot" && props.turn()!.retry == null}>
          <div class="lk-pill-tail">
            已经超过 3 分钟没有新事件。codex 内置 web_search 阶段确实可能这么久 ——
            如果你不想等,请点击右上方的中止按钮。
          </div>
        </Show>
      </div>
    </Show>
  );
}
