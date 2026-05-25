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

// ── M3.5 retry button + new kinds ────────────────────────────────────

/** Mirrors Chat.tsx::priorUserContent — given an assistant message
 *  sequence, walks back through `messages` and returns the content of
 *  the most recent preceding `role === "user"` message. Returns null
 *  if no user message precedes. */
function priorUserContent(messages, assistantSeq) {
  const idx = messages.findIndex((m) => m.seq === assistantSeq);
  if (idx <= 0) return null;
  for (let i = idx - 1; i >= 0; i--) {
    if (messages[i].role === "user") return messages[i].content;
  }
  return null;
}

// 9. priorUserContent returns the immediately-preceding user message —
//    the normal turn shape (user → assistant).
{
  const msgs = [
    { seq: 1, role: "user", content: "What is X?" },
    { seq: 2, role: "assistant", content: "" /* fatal turn — empty reply */ },
  ];
  assert.equal(priorUserContent(msgs, 2), "What is X?");
}

// 10. priorUserContent skips assistant messages and finds the prior user
//     in a multi-turn session.
{
  const msgs = [
    { seq: 1, role: "user", content: "first" },
    { seq: 2, role: "assistant", content: "ok" },
    { seq: 3, role: "user", content: "second" },
    { seq: 4, role: "assistant", content: "" /* fatal */ },
  ];
  assert.equal(priorUserContent(msgs, 4), "second");
}

// 11. priorUserContent returns null when no user message precedes (an
//     unusual shape — but the renderer should hide the retry button
//     rather than crash on null).
{
  const msgs = [{ seq: 1, role: "assistant", content: "orphan" }];
  assert.equal(priorUserContent(msgs, 1), null);
}

// 12. priorUserContent returns null when the assistant seq is not found
//     (defensive — store could in principle race the message list).
{
  const msgs = [
    { seq: 1, role: "user", content: "x" },
    { seq: 2, role: "assistant", content: "y" },
  ];
  assert.equal(priorUserContent(msgs, 999), null);
}

// 13. M3.5 new fatal kinds — CodexStreamTimeout (T31b crash signature).
{
  const payload = {
    stop_reason: "fatal_error",
    fatal_reason: {
      kind: "codex_stream_timeout",
      detail: "codex SSE 流中段超时：reading SSE chunk → operation timed out",
      hint:
        "codex SSE 流中段超时（reading SSE chunk → timed out），通常网络抖动 / codex 临时过载，重试可恢复。（已自动重试 4 次，仍失败）",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr);
  assert.equal(fr.kind, "codex_stream_timeout");
  assert.match(fr.hint, /中段超时/);
  assert.match(fr.hint, /已自动重试 4 次/);
}

// 14. M3.5 new fatal kinds — CodexStreamDecodeError (transient UTF-8 / JSON
//     split on a chunk boundary).
{
  const payload = {
    stop_reason: "fatal_error",
    fatal_reason: {
      kind: "codex_stream_decode_error",
      detail: "codex SSE 流解码失败：non-UTF-8 in SSE event",
      hint: "codex SSE 流字节损坏 / 分块边界异常，重试通常能拿到一份干净的 stream。",
    },
  };
  const fr = parseFatalReason(payload);
  assert.ok(fr);
  assert.equal(fr.kind, "codex_stream_decode_error");
  assert.match(fr.hint, /干净的 stream/);
}

// 15. Retry button enablement: the helper resolves a prior user content
//     for the assistant message, so the button is shown. Disabled until
//     `sending = false`. Sanity assertion only — JSX-level test would
//     require a full component harness, but the contract here is that
//     priorUserContent + the parent's `sending` callback are the inputs.
{
  const msgs = [
    { seq: 10, role: "user", content: "Long deep-dive prompt" },
    { seq: 11, role: "assistant", content: "" },
  ];
  const content = priorUserContent(msgs, 11);
  assert.equal(content, "Long deep-dive prompt");
  const sending = false;
  // Mirror the renderer's disabled binding.
  const buttonDisabled = sending;
  assert.equal(buttonDisabled, false);
}

console.log("fatal_render tests passed");
