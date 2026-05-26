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

// ── shape mirror: applyLifecycle subagent gate (M3.7 follow-up) ──────
// The composer footer ("回合进行中 · 正在启动…") reads
// `turns.find(t => t.status === "running")`. Before the fix,
// `applyLifecycle` called `ensureTurn(d, turn_id)` even when the event
// was emitted from a subagent loop (turn_id like
// "turn-abc.sub-deadbeef"), creating a Turn entry that never received
// an `assistant_done` to flip it to "done" — so every replayed session
// showed the pill forever. The gate skips lifecycle events with
// `parent_turn_id` set, because the subagent's per-turn state is
// already surfaced via the subagent_card on the parent turn.

function lifecycleShouldCreateTurn(payload) {
  // Returns true if the lifecycle event should be allowed to call
  // ensureTurn(turn_id). Mirrors the gate at the top of
  // store.applyLifecycle.
  if (!payload || !payload.turn_id) return false;
  if (payload.parent_turn_id != null) return false;
  return true;
}

// ── shape mirror: subagent turn-id helpers (M3.7 follow-up) ─────────
// The depth-2+ subagent rendering bug: `applyCanvas` called
// `ensureTurn(d, parentTurnId)` with a parentTurnId that was itself a
// sub-turn id (depth ≥ 2 subagents nest), minting a stray Turn entry
// that became "回合 2 进行中" forever. The fix introduces
// topLevelTurnId() to strip back to the main turn, and
// findSubagentInTree() to walk the subagent_card chain.

function topLevelTurnId(turnId) {
  const idx = turnId.indexOf(".sub-");
  return idx < 0 ? turnId : turnId.slice(0, idx);
}

function findSubagentInTree(turn, subagentTurnId) {
  function walk(arts) {
    for (const a of arts) {
      if (a.kind === "subagent" && a.subagentTurnId === subagentTurnId) return a;
      if (a.kind === "subagent" && a.innerArtifacts) {
        const inner = walk(a.innerArtifacts);
        if (inner) return inner;
      }
    }
    return null;
  }
  return walk(turn.artifacts);
}

console.log("\ntopLevelTurnId:");
check("main turn passes through unchanged", () => {
  assert.equal(topLevelTurnId("turn-abc123"), "turn-abc123");
});
check("depth-1 sub-turn strips the .sub- suffix", () => {
  assert.equal(
    topLevelTurnId("turn-abc123.sub-deadbeef"),
    "turn-abc123",
  );
});
check("depth-2 nested sub-turn strips ALL suffixes", () => {
  assert.equal(
    topLevelTurnId("turn-abc123.sub-deadbeef.sub-cafebabe"),
    "turn-abc123",
  );
});
check("depth-3 nested sub-turn strips ALL suffixes", () => {
  assert.equal(
    topLevelTurnId("turn-abc.sub-d.sub-e.sub-f"),
    "turn-abc",
  );
});

console.log("\nfindSubagentInTree:");
check("finds depth-1 card directly on main turn", () => {
  const turn = {
    artifacts: [
      { kind: "subagent", subagentTurnId: "turn-X.sub-D1", innerArtifacts: [] },
      { kind: "tool", artifactId: "t1" },
    ],
  };
  const found = findSubagentInTree(turn, "turn-X.sub-D1");
  assert.ok(found && found.kind === "subagent");
  assert.equal(found.subagentTurnId, "turn-X.sub-D1");
});
check("walks into innerArtifacts to find depth-2 card", () => {
  const turn = {
    artifacts: [
      {
        kind: "subagent",
        subagentTurnId: "turn-X.sub-D1",
        innerArtifacts: [
          { kind: "tool", artifactId: "t1" },
          { kind: "subagent", subagentTurnId: "turn-X.sub-D1.sub-D2a", innerArtifacts: [] },
          { kind: "subagent", subagentTurnId: "turn-X.sub-D1.sub-D2b", innerArtifacts: [] },
        ],
      },
    ],
  };
  const found = findSubagentInTree(turn, "turn-X.sub-D1.sub-D2b");
  assert.ok(found);
  assert.equal(found.subagentTurnId, "turn-X.sub-D1.sub-D2b");
});
check("walks recursively for depth-3", () => {
  const turn = {
    artifacts: [
      {
        kind: "subagent",
        subagentTurnId: "turn-X.sub-A",
        innerArtifacts: [
          {
            kind: "subagent",
            subagentTurnId: "turn-X.sub-A.sub-B",
            innerArtifacts: [
              { kind: "subagent", subagentTurnId: "turn-X.sub-A.sub-B.sub-C", innerArtifacts: [] },
            ],
          },
        ],
      },
    ],
  };
  const found = findSubagentInTree(turn, "turn-X.sub-A.sub-B.sub-C");
  assert.ok(found);
  assert.equal(found.subagentTurnId, "turn-X.sub-A.sub-B.sub-C");
});
check("returns null when no match", () => {
  const turn = {
    artifacts: [{ kind: "subagent", subagentTurnId: "turn-X.sub-Z", innerArtifacts: [] }],
  };
  assert.equal(findSubagentInTree(turn, "turn-X.sub-MISSING"), null);
});
check("returns null on empty turn", () => {
  assert.equal(findSubagentInTree({ artifacts: [] }, "anything"), null);
});

console.log("\napplyLifecycle subagent gate:");
check("main-agent assistant_done passes through", () => {
  assert.equal(
    lifecycleShouldCreateTurn({
      turn_id: "turn-abc123",
      stop_reason: "end_turn",
    }),
    true,
  );
});
check("subagent turn_cost_capped (parent_turn_id set) is skipped", () => {
  // Real shape from a DR4-style cost-capped subagent — the event
  // surface=lifecycle and parent_turn_id points at the spawning turn.
  assert.equal(
    lifecycleShouldCreateTurn({
      turn_id: "turn-abc123.sub-deadbeef",
      parent_turn_id: "turn-abc123",
      cap_usd: 5.0,
      actual_cost_usd: 5.018,
    }),
    false,
  );
});
check("subagent provider_retry_attempt is skipped", () => {
  assert.equal(
    lifecycleShouldCreateTurn({
      turn_id: "turn-abc123.sub-deadbeef",
      parent_turn_id: "turn-abc123",
      attempt: 1,
      max_attempts: 5,
      kind: "codex_stream_silent",
    }),
    false,
  );
});
check("missing turn_id never creates an entry", () => {
  assert.equal(lifecycleShouldCreateTurn({}), false);
  assert.equal(lifecycleShouldCreateTurn(null), false);
});

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
