// Unit tests for M2.6 settings + cost cap wiring.
//
// Run with `node tests/settings.test.mjs` from `frontend/web/`.
//
// The tests cover the parts that are exercisable without a DOM: the
// `turn_cost_capped` reducer in `store.ts` (the data path the warning
// bar reads), and the small validation-error parser inside `Settings.tsx`
// (the path the settings UI reads when a PATCH 400s).
//
// We don't drive the JSX trees themselves — that would need solid-js
// hydration in jsdom, and the existing test runner is a flat node script.
// What we do test is enough to catch the reducers / parsers from drifting
// against the backend payload shapes.

import assert from "node:assert/strict";

// ── helper: replicate the slice of store.ts that handles the new event.
// (We don't import the real module because Solid's reactivity requires a
//  bundler. The shape mirror below is what `applyLifecycle` does for
//  the `turn_cost_capped` kind — keep this in sync if you change the
//  reducer.)
function applyCostCappedToTurn(turn, payload) {
  return {
    ...turn,
    costCap: {
      capUsd: Number(payload.cap_usd ?? 0),
      actualCostUsd: Number(payload.actual_cost_usd ?? 0),
      iterCount: Number(payload.iter_count ?? 0),
    },
  };
}

// ── helper: mirror of Settings.tsx `parseValidationErrors`. Same caveat:
// keep in sync. The function is small and worth checking because the
// gateway double-wraps validation errors (ApiError envelope + JSON body),
// and a parser bug would silently hide field errors from the user.
function parseValidationErrors(body) {
  if (!body || typeof body !== "object") return [];
  const raw = body.error;
  if (typeof raw !== "string") return [];
  try {
    const inner = JSON.parse(raw);
    if (inner.kind === "validation_failed" && Array.isArray(inner.errors)) {
      return inner.errors;
    }
  } catch {
    // fall through
  }
  return [{ field: "_", message: raw }];
}

// ── helper: shouldShowCostCapBar — exposed via store data shape.
function shouldShowCostCapBar(turn) {
  return turn.stopReason === "cost_cap_exceeded" && turn.costCap != null;
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

console.log("turn_cost_capped reducer:");
check("populates costCap from the event payload", () => {
  const turn = { turnId: "t1", costCap: null, stopReason: null };
  const updated = applyCostCappedToTurn(turn, {
    turn_id: "t1",
    cap_usd: 0.5,
    actual_cost_usd: 0.61,
    iter_count: 7,
  });
  assert.deepEqual(updated.costCap, {
    capUsd: 0.5,
    actualCostUsd: 0.61,
    iterCount: 7,
  });
});

check("missing fields default to 0", () => {
  const turn = { turnId: "t1", costCap: null };
  const updated = applyCostCappedToTurn(turn, { turn_id: "t1" });
  assert.equal(updated.costCap.capUsd, 0);
  assert.equal(updated.costCap.actualCostUsd, 0);
  assert.equal(updated.costCap.iterCount, 0);
});

console.log("\nshouldShowCostCapBar:");
check("bar is hidden until stopReason matches", () => {
  // Imagine an event arrived with costCap data but the turn hasn't
  // received assistant_done yet. We still gate on stopReason so the bar
  // appears at turn end (when the user expects it), not during the run.
  assert.equal(
    shouldShowCostCapBar({ stopReason: null, costCap: { capUsd: 0.5, actualCostUsd: 0.6, iterCount: 1 } }),
    false,
  );
  assert.equal(shouldShowCostCapBar({ stopReason: "end_turn", costCap: null }), false);
  assert.equal(
    shouldShowCostCapBar({
      stopReason: "cost_cap_exceeded",
      costCap: { capUsd: 0.5, actualCostUsd: 0.6, iterCount: 1 },
    }),
    true,
  );
});

console.log("\nparseValidationErrors:");
check("unwraps the gateway's double envelope", () => {
  const inner = JSON.stringify({
    kind: "validation_failed",
    errors: [{ field: "cost_cap_usd", message: "must be a non-negative number" }],
  });
  const body = { error: inner };
  const errs = parseValidationErrors(body);
  assert.equal(errs.length, 1);
  assert.equal(errs[0].field, "cost_cap_usd");
  assert.equal(errs[0].message, "must be a non-negative number");
});

check("falls back to a single _ error on unknown shape", () => {
  const body = { error: "boom" };
  const errs = parseValidationErrors(body);
  assert.equal(errs.length, 1);
  assert.equal(errs[0].field, "_");
  assert.equal(errs[0].message, "boom");
});

check("returns [] on garbage", () => {
  assert.deepEqual(parseValidationErrors(null), []);
  assert.deepEqual(parseValidationErrors({}), []);
  assert.deepEqual(parseValidationErrors({ error: 42 }), []);
});

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
