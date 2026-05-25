// TurnStatusPill — M3.2.
//
// Sits above the chat composer while a turn is in flight. Tells the user
// the agent is still alive even when the codex reasoning + builtin
// web_search phase emits no `assistant_delta` for minutes at a time
// (the M3 deep-review pain point: 4 minutes of dead chat, user thinks
// gateway is stuck). Two affordances:
//
//   1. A live status line (iteration / elapsed / what the agent is doing)
//      that updates once a second from a local timer.
//   2. A 强制停止本回合 button that POSTs to the abort endpoint and trusts
//      the SSE stream (`assistant_done` with stop_reason="user_aborted")
//      to dismiss the pill.
//
// The pill never claims progress it can't measure — there is no "X%
// done" because the codex reasoning step has no progress signal. The
// honest framing is "时间到了 X 分钟，可能还在深度思考" — slow is fine
// as long as the user knows we're not stuck.

import { createEffect, createSignal, onCleanup, Show } from "solid-js";

import type { Turn, TurnActivity } from "./types";

type Props = {
  /** The currently running turn, or null when no turn is in flight. */
  turn: () => Turn | null;
  /** Session id for the abort POST. */
  sessionId: () => string | null;
};

/** Render an elapsed-ms value as "Xs" / "Xm Ys". A turn longer than
 *  60 min reads as e.g. "63m 12s" — we don't break to hours because
 *  that crossing belongs in the wall-clock guard, not the pill. */
function fmtElapsed(ms: number): string {
  if (ms < 0) ms = 0;
  const totalSec = Math.floor(ms / 1000);
  if (totalSec < 60) return `${totalSec}秒`;
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m}分${s.toString().padStart(2, "0")}秒`;
}

/** The "currently doing" line — exported for unit tests. */
export function describeActivity(
  activity: TurnActivity | null,
  elapsedMs: number,
): string {
  if (activity?.kind === "tool") return `正在调用 ${activity.displayName}`;
  if (activity?.kind === "search") return "正在搜索网页（codex 内置）";
  if (activity?.kind === "delta") return "正在写回答";
  // No canvas signal yet — fall through to time-based rungs.
  if (elapsedMs >= 60_000) {
    return "深度思考中（可能在 codex 内部多次搜索，不会有进度反馈）";
  }
  if (elapsedMs >= 5_000) return "agent 正在思考…";
  return "正在启动…";
}

/** Color / urgency rung from elapsed-since-last-signal time — exported
 *  so the test suite can pin the thresholds. Returns a class suffix the
 *  CSS picks up: `"calm"` / `"warm"` / `"hot"`. */
export function urgencyLevel(elapsedSinceSignalMs: number): "calm" | "warm" | "hot" {
  if (elapsedSinceSignalMs >= 180_000) return "hot";
  if (elapsedSinceSignalMs >= 60_000) return "warm";
  return "calm";
}

export function TurnStatusPill(props: Props) {
  const [nowMs, setNowMs] = createSignal(Date.now());
  const [aborting, setAborting] = createSignal(false);
  // Last time we saw ANY signal-bearing event (canvas activity flip or
  // the turn first appearing). Used for urgency-color thresholds — we
  // dim the pill yellow at 60s / brighter at 180s, but ONLY if there's
  // been no fresh signal in that window. A tool call resets it.
  const [lastSignalAtMs, setLastSignalAtMs] = createSignal(Date.now());
  let prevActivityKey: string | null = null;
  let prevTurnId: string | null = null;

  // Re-arm the signal clock whenever the activity changes (kind or
  // display name flips, like a new tool starting). Solid's createEffect
  // runs after props.turn() changes, so this naturally tracks event
  // arrival.
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

  // Iteration count comes off the latest artifact (artifacts carry the
  // iteration that produced them). Fallback to "0" before the first
  // event — the pill still shows "正在启动" then.
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

  const elapsedSinceSignalMs = () => nowMs() - lastSignalAtMs();
  const urgency = () => urgencyLevel(elapsedSinceSignalMs());

  const onAbort = async () => {
    const t = props.turn();
    const sid = props.sessionId();
    if (!t || !sid || aborting()) return;
    // Browser confirm is honest: no decorative custom modal, plain
    // browser prompt that the user understands. The button is rare
    // enough that an extra confirm click does not chafe.
    if (!window.confirm("中止本回合？已收到的文字会保留。")) return;
    setAborting(true);
    try {
      const res = await fetch(
        `/api/v1/sessions/${sid}/turns/${t.turnId}/abort`,
        { method: "POST" },
      );
      if (!res.ok && res.status !== 404) {
        // 404 means the turn just finished on its own — treat as a no-op.
        // Anything else: surface to console; we don't block the user
        // because the SSE assistant_done will arrive anyway.
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
          "turn-pill": true,
          [`pill-${urgency()}`]: true,
          // M3.4: a separate class while a retry is in flight so the
          // CSS can dim the regular activity text and highlight the
          // retry indicator instead.
          "pill-retrying": props.turn()!.retry != null,
        }}
        role="status"
        aria-live="polite"
      >
        <div class="pill-line">
          <span class="pill-spinner" aria-hidden="true">⟳</span>
          <span class="pill-text">
            <span class="pill-iter">回合进行中 · iter {Math.max(1, iteration())}</span>
            <span class="pill-sep">·</span>
            <span class="pill-elapsed">已用 {fmtElapsed(elapsedMs())}</span>
            <span class="pill-sep">·</span>
            <Show
              when={props.turn()!.retry != null}
              fallback={
                <span class="pill-activity">
                  {describeActivity(props.turn()!.activity, elapsedMs())}
                </span>
              }
            >
              <span class="pill-retry" title={describeRetryKind(props.turn()!.retry!.kind)}>
                重试中 ({props.turn()!.retry!.attempt}/
                {props.turn()!.retry!.maxAttempts})…
                <Show when={props.turn()!.retry!.backoffMs > 0}>
                  <span class="pill-retry-backoff">
                    {" "}等 {Math.round(props.turn()!.retry!.backoffMs / 1000)} 秒
                  </span>
                </Show>
              </span>
            </Show>
          </span>
          <button
            class="pill-abort"
            onClick={() => void onAbort()}
            disabled={aborting()}
            title="向 agent 发起中止信号"
          >
            {aborting() ? "正在停止…" : "强制停止本回合"}
          </button>
        </div>
        <Show when={urgency() === "hot" && props.turn()!.retry == null}>
          <div class="pill-tail">
            已经超过 3 分钟没有新事件。codex 内置 web_search 阶段确实可能这么久 ——
            如果你不想等，请点击右上方的中止按钮。
          </div>
        </Show>
      </div>
    </Show>
  );
}

/** M3.4: human-readable tooltip for the retry-kind tag. The pill body
 *  stays short ("重试中 (N/M)…"); the kind goes on hover so a curious
 *  user can tell a 5xx burst from a connection blip. */
function describeRetryKind(kind: string): string {
  switch (kind) {
    case "codex_http_5xx":
      return "codex 服务端返回 5xx，正在重试";
    case "codex_connection_failed":
      return "无法连接 codex（DNS / 网络抖动），正在重试";
    case "codex_stream_silent":
      return "codex 流静默，正在重试";
    default:
      return `provider 错误（${kind}），正在重试`;
  }
}
