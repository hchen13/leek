// Fatal-reason hint card (M3.3) unit tests.
//
// Same pattern as activation.test.mjs / settings.test.mjs / etc.: re-
// implement the pure helpers verbatim here and assert their shape. The
// helpers we care about:
//
//   parseFatalReason(payload) — extracts {kind, detail, hint} from the
//                               assistant_done payload (mirrors the
//                               applyLifecycle slice in store.ts).
//   shouldShowFatalCard(turn) — gate the hint card on the chat surface
//                               (mirrors the same-name helper in Chat.tsx).
//
// Run with `node tests/fatal_render.test.mjs` from `frontend/web/`.

import assert from "node:assert/strict";

// ── helpers (verbatim from src/store.ts and src/Chat.tsx) ────────────

/** Mirrors the assistant_done branch in store.ts::applyLifecycle —
 *  pulls the typed fatal_reason payload off the event payload and
 *  returns a TurnFatalReason or null. */
function parseFatalReason(eventPayload) {
  const fr = eventPayload.fatal_reason;
  if (!fr || typeof fr !== "object") return null;
  const obj = fr;
  const kind = obj.kind != null ? String(obj.kind) : "";
  const detail = obj.detail != null ? String(obj.detail) : "";
  const hint = obj.hint != null ? String(obj.hint) : "";
  if (!kind) return null;
  return { kind, detail, hint };
}

/** Mirrors Chat.tsx::shouldShowFatalCard — the hint card renders iff a
 *  typed reason is present on the turn. */
function shouldShowFatalCard(turn) {
  return turn.fatalReason != null;
}

// ── tests ────────────────────────────────────────────────────────────

// 1. A natural assistant_done (no fatal_reason) yields null — the hint
//    card does not render on end_turn.
{
  const payload = {
    turn_id: "t",
    message_seq: 1,
    stop_reason: "end_turn",
  };
  assert.equal(parseFatalReason(payload), null);
  assert.equal(shouldShowFatalCard({ fatalReason: null }), false);
}

// 2. A fatal_error with a CodexHttp5xx reason renders the hint card.
{
  const payload = {
    turn_id: "t",
    message_seq: 2,
    stop_reason: "fatal_error",
    fatal_reason: {
      kind: "codex_http_5xx",
      detail: "codex 返回 HTTP 503：service unavailable",
      hint: "codex 服务端临时不可用，稍后重试通常能解决。",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr, "should parse");
  assert.equal(fr.kind, "codex_http_5xx");
  assert.match(fr.detail, /503/);
  assert.match(fr.hint, /稍后重试/);
  assert.equal(shouldShowFatalCard({ fatalReason: fr }), true);
}

// 3. A 4xx (token expired) reason carries a different actionable hint.
{
  const payload = {
    stop_reason: "fatal_error",
    fatal_reason: {
      kind: "codex_http_4xx",
      detail: "codex 返回 HTTP 401",
      hint: "请求被 codex 拒绝（可能 token 失效 / 请求超大），建议检查 Settings 里 token 或缩短 prompt 重试。",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr);
  assert.equal(fr.kind, "codex_http_4xx");
  assert.match(fr.hint, /token/);
}

// 4. CodexStreamSilent — fired by the substantive-only idle detector
//    (stop_reason == "idle_timeout", but the payload carries the typed
//    reason so the hint card still renders). The hint includes the
//    silent_secs number so the user sees which threshold tripped.
{
  const payload = {
    stop_reason: "idle_timeout",
    fatal_reason: {
      kind: "codex_stream_silent",
      detail: "codex 连接静默 90 秒被超时杀",
      hint: "codex 连接静默 90 秒被超时杀，可能网络抖动 / codex 临时过载，重试即可。",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr);
  assert.equal(fr.kind, "codex_stream_silent");
  assert.match(fr.detail, /90/);
  assert.match(fr.hint, /重试/);
}

// 5. ConnectionFailed — DNS / TCP / TLS errors.
{
  const payload = {
    stop_reason: "fatal_error",
    fatal_reason: {
      kind: "codex_connection_failed",
      detail: "无法连接 codex：dns lookup failed",
      hint: "无法连接 codex 服务（DNS / 网络），检查网络后重试。",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr);
  assert.equal(fr.kind, "codex_connection_failed");
  assert.match(fr.hint, /网络/);
}

// 6. Malformed — SSE parse failure, "this is a leek bug" message.
{
  const payload = {
    stop_reason: "fatal_error",
    fatal_reason: {
      kind: "codex_malformed",
      detail: "codex 返回格式异常：parsing SSE data as JSON → non-UTF-8 in SSE event",
      hint: "codex 返回的格式 leek 没处理过，这是 leek 的 bug，请把这个 turn id 告诉开发者。",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr);
  assert.equal(fr.kind, "codex_malformed");
  assert.match(fr.hint, /bug/);
}

// 7. Payload with kind empty / missing → returns null (defensive — the
//    renderer keys off kind, so an empty payload shouldn't materialize
//    an empty card).
{
  const noKind = {
    stop_reason: "fatal_error",
    fatal_reason: { kind: "", detail: "x", hint: "y" },
  };
  assert.equal(parseFatalReason(noKind), null);

  const missing = {
    stop_reason: "fatal_error",
    fatal_reason: { detail: "x", hint: "y" },
  };
  assert.equal(parseFatalReason(missing), null);
}

// 8. Non-object fatal_reason value (defensive — bad backend would not
//    ship this, but parseFatalReason should not crash either).
{
  const stringy = { stop_reason: "fatal_error", fatal_reason: "oops" };
  assert.equal(parseFatalReason(stringy), null);

  const nully = { stop_reason: "fatal_error", fatal_reason: null };
  assert.equal(parseFatalReason(nully), null);
}

console.log("fatal_render tests passed");
