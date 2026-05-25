// TurnStatusPill (M3.2) unit tests.
//
// The pill itself is JSX — driving it would need solid-js + jsdom + a
// bundler. We don't have that infrastructure. What IS pure and worth
// testing is the two helpers the renderer leans on:
//
//   describeActivity(activity, elapsedMs) — the status line text
//   urgencyLevel(elapsedSinceSignalMs)    — the color rung
//
// We re-implement them verbatim here (the same pattern as
// activation.test.mjs / settings.test.mjs — keep in sync with
// src/TurnStatusPill.tsx). The cost of duplication is small; the
// alternative (full Solid hydration in jsdom) is large.
//
// Run with `node tests/turn_status_pill.test.mjs` from `frontend/web/`.

import assert from "node:assert/strict";

// ── helpers (verbatim from src/TurnStatusPill.tsx) ───────────────────

function describeActivity(activity, elapsedMs) {
  if (activity?.kind === "tool") return `正在调用 ${activity.displayName}`;
  if (activity?.kind === "search") return "正在搜索网页（codex 内置）";
  if (activity?.kind === "delta") return "正在写回答";
  if (elapsedMs >= 60_000) {
    return "深度思考中（可能在 codex 内部多次搜索，不会有进度反馈）";
  }
  if (elapsedMs >= 5_000) return "agent 正在思考…";
  return "正在启动…";
}

function urgencyLevel(elapsedSinceSignalMs) {
  if (elapsedSinceSignalMs >= 180_000) return "hot";
  if (elapsedSinceSignalMs >= 60_000) return "warm";
  return "calm";
}

// ── shape mirror: applyCanvas activity-rollover rule ─────────────────
// Replicates the slice of store.ts that flips `turn.activity` based on
// a canvas event's `(kind, phase)`. The pill renderer reads
// `turn.activity` and runs it through `describeActivity` — drift here
// breaks "正在调用 corpus_search" appearing when the user fires a tool.

function rollActivity(prevActivity, event) {
  // event: { kind: 'tool'|'search', phase: 'start'|'completion'|'error',
  //          data: { display_name, tool } }
  if (event.kind === "tool" || event.kind === "search") {
    if (event.phase === "start") {
      if (event.kind === "tool") {
        return {
          kind: "tool",
          displayName:
            event.data?.display_name ?? event.data?.tool ?? "工具",
        };
      }
      return { kind: "search" };
    }
    // completion / error of the same kind clears (so the pill drops
    // back to time-based rung until the next event).
    if (prevActivity?.kind === event.kind) return null;
  }
  return prevActivity;
}

let pass = 0;
let fail = 0;
function check(name, fn) {
  try {
    fn();
    pass += 1;
    console.log(`  ok  ${name}`);
  } catch (e) {
    fail += 1;
    console.log(`  FAIL ${name}: ${e.message}`);
  }
}

console.log("describeActivity:");
check("tool reads off display name", () => {
  assert.equal(
    describeActivity({ kind: "tool", displayName: "corpus_search" }, 1000),
    "正在调用 corpus_search",
  );
});
check("search shows builtin-codex hint", () => {
  assert.equal(
    describeActivity({ kind: "search" }, 1000),
    "正在搜索网页（codex 内置）",
  );
});
check("delta shows writing-answer", () => {
  assert.equal(describeActivity({ kind: "delta" }, 1000), "正在写回答");
});
check("no activity, t<5s → starting", () => {
  assert.equal(describeActivity(null, 2_000), "正在启动…");
});
check("no activity, 5s <= t < 60s → thinking", () => {
  assert.equal(describeActivity(null, 30_000), "agent 正在思考…");
});
check("no activity, t >= 60s → deep thinking", () => {
  assert.equal(
    describeActivity(null, 90_000),
    "深度思考中（可能在 codex 内部多次搜索，不会有进度反馈）",
  );
});
check("activity beats time — tool wins even at 5 min", () => {
  // A tool call in flight is the strongest signal — we don't switch to
  // "deep thinking" just because the turn has been long.
  assert.equal(
    describeActivity({ kind: "tool", displayName: "get_financials" }, 300_000),
    "正在调用 get_financials",
  );
});

console.log("\nurgencyLevel:");
check("<60s → calm", () => {
  assert.equal(urgencyLevel(0), "calm");
  assert.equal(urgencyLevel(45_000), "calm");
  assert.equal(urgencyLevel(59_999), "calm");
});
check("60s..180s → warm", () => {
  assert.equal(urgencyLevel(60_000), "warm");
  assert.equal(urgencyLevel(120_000), "warm");
  assert.equal(urgencyLevel(179_999), "warm");
});
check(">=180s → hot", () => {
  assert.equal(urgencyLevel(180_000), "hot");
  assert.equal(urgencyLevel(600_000), "hot");
});

console.log("\nrollActivity (store.applyCanvas slice):");
check("tool start sets activity, completion clears", () => {
  let a = null;
  a = rollActivity(a, {
    kind: "tool",
    phase: "start",
    data: { display_name: "corpus_search" },
  });
  assert.deepEqual(a, { kind: "tool", displayName: "corpus_search" });
  a = rollActivity(a, {
    kind: "tool",
    phase: "completion",
    data: {},
  });
  assert.equal(a, null);
});
check("search start sets, completion clears", () => {
  let a = rollActivity(null, { kind: "search", phase: "start", data: {} });
  assert.deepEqual(a, { kind: "search" });
  a = rollActivity(a, { kind: "search", phase: "completion", data: {} });
  assert.equal(a, null);
});
check("tool start uses internal name if display_name missing", () => {
  const a = rollActivity(null, {
    kind: "tool",
    phase: "start",
    data: { tool: "use_skill" },
  });
  assert.deepEqual(a, { kind: "tool", displayName: "use_skill" });
});
check("fallback to '工具' if both missing", () => {
  const a = rollActivity(null, {
    kind: "tool",
    phase: "start",
    data: {},
  });
  assert.deepEqual(a, { kind: "tool", displayName: "工具" });
});
check("completion of a different kind does not clear", () => {
  // A tool is in flight; a search completion arrives (different kind).
  // We should leave the tool activity alone so the pill keeps reading
  // "正在调用 X" until the tool itself finishes.
  let a = { kind: "tool", displayName: "X" };
  a = rollActivity(a, { kind: "search", phase: "completion", data: {} });
  assert.deepEqual(a, { kind: "tool", displayName: "X" });
});

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
