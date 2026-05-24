// Activation state-machine test for the Corpus Brain (M2.1).
//
// The activation snapshot is a per-node ref-count of `liveTurns` and
// `completedTurns`, plus a `currentTurn` cursor (the most recent
// active turn). `activationLevelOf` resolves a node to one of
// `live` / `turn` / `session` / `historical` from that snapshot.
//
// This test re-implements the two pure helpers from `src/store.ts`
// (we can't `import` a .ts file from a node script directly) and
// drives them through a sequence of synthetic SSE events that mirror
// what the gateway emits during a real turn. The goal is to catch
// regressions in the level-derivation rules, not to test Solid wiring.
//
// Run with `node tests/activation.test.mjs` from `frontend/web/`.

// ── helpers (verbatim from src/store.ts) ─────────────────────────────

function corpusIdsFromEvent(ev) {
  const data = (ev.payload.data ?? {});
  const tool = String(data.tool ?? "");
  const dp = (data.display_payload ?? {});
  if (tool === "corpus_read") {
    const id = dp.id;
    return typeof id === "string" && id ? [id] : [];
  }
  if (tool === "corpus_search") {
    const hits = Array.isArray(dp.hits) ? dp.hits : [];
    return hits.map((h) => String(h.id ?? "")).filter((s) => s.length > 0);
  }
  return [];
}

function activationLevelOf(activation, nodeId) {
  const ref = activation.byNode.get(nodeId);
  if (!ref) return activation.historical.has(nodeId) ? "historical" : null;
  if (activation.currentTurn && ref.liveTurns.has(activation.currentTurn)) return "live";
  if (activation.currentTurn && ref.completedTurns.has(activation.currentTurn)) return "turn";
  if (ref.completedTurns.size > 0) return "session";
  return activation.historical.has(nodeId) ? "historical" : null;
}

function emptyActivation() {
  return { byNode: new Map(), currentTurn: null, historical: new Set() };
}

/** Apply one `tool_lifecycle` event to the activation snapshot. Mirrors
 *  the relevant branch of `applyCanvas` in src/store.ts. */
function applyToolLifecycle(act, ev) {
  const ids = corpusIdsFromEvent(ev);
  if (ids.length === 0) return act;
  const phase = ev.payload.phase;
  const turnId = String(ev.payload.turn_id);
  act.currentTurn = turnId;
  for (const id of ids) {
    const ref = act.byNode.get(id) ?? {
      liveTurns: new Set(),
      completedTurns: new Set(),
    };
    if (phase === "start") {
      ref.liveTurns.add(turnId);
    } else {
      ref.liveTurns.delete(turnId);
      ref.completedTurns.add(turnId);
    }
    act.byNode.set(id, ref);
  }
  return act;
}

// ── synthetic event factories ────────────────────────────────────────

function ev(turnId, tool, phase, payload) {
  return {
    seq: 1,
    kind: "tool_lifecycle",
    payload: {
      turn_id: turnId,
      phase,
      data: { tool, display_payload: payload },
    },
    created_at: "2026-05-24T00:00:00Z",
  };
}

const searchStart = (turnId) => ev(turnId, "corpus_search", "start", {});
const searchDone = (turnId, ids) =>
  ev(turnId, "corpus_search", "completion", { hits: ids.map((id) => ({ id })) });
const readStart = (turnId, id) => ev(turnId, "corpus_read", "start", { id });
const readDone = (turnId, id) => ev(turnId, "corpus_read", "completion", { id });

// ── test runner ──────────────────────────────────────────────────────

let pass = 0;
let fail = 0;
function check(name, predicate) {
  let ok = false;
  let err = "";
  try {
    ok = !!predicate();
  } catch (e) {
    err = String(e?.message ?? e);
  }
  if (ok) {
    pass += 1;
    console.log(`  ok  ${name}`);
  } else {
    fail += 1;
    console.log(`  FAIL ${name}${err ? ` — ${err}` : ""}`);
  }
}

// ── tests ────────────────────────────────────────────────────────────

console.log("activation.state machine:");

const NODE_A = "wikis/principles/concepts/circle-of-competence.md";
const NODE_B = "wikis/principles/concepts/margin-of-safety.md";
const NODE_C = "wikis/knowledge/concepts/example.md";

// 1) Empty snapshot — no activation anywhere.
{
  const a = emptyActivation();
  check("empty snapshot returns null for unknown id", () => activationLevelOf(a, NODE_A) === null);
}

// 2) corpus_read start → "live" on that turn.
{
  const a = emptyActivation();
  applyToolLifecycle(a, readStart("turn-1", NODE_A));
  check("corpus_read start sets node to 'live'", () => activationLevelOf(a, NODE_A) === "live");
  check("currentTurn advances to the calling turn", () => a.currentTurn === "turn-1");
}

// 3) corpus_read start → completion → "turn".
{
  const a = emptyActivation();
  applyToolLifecycle(a, readStart("turn-1", NODE_A));
  applyToolLifecycle(a, readDone("turn-1", NODE_A));
  check("start→completion demotes 'live' → 'turn'", () => activationLevelOf(a, NODE_A) === "turn");
}

// 4) Turn rolls forward — a previously-completed node falls to "session".
{
  const a = emptyActivation();
  applyToolLifecycle(a, readStart("turn-1", NODE_A));
  applyToolLifecycle(a, readDone("turn-1", NODE_A));
  // New turn touches a different wiki — currentTurn moves.
  applyToolLifecycle(a, readStart("turn-2", NODE_B));
  check("node from a previous turn falls to 'session'", () => activationLevelOf(a, NODE_A) === "session");
  check("the just-started node is 'live' in turn-2", () => activationLevelOf(a, NODE_B) === "live");
}

// 5) corpus_search completion fans out across every hit.
{
  const a = emptyActivation();
  applyToolLifecycle(a, searchStart("turn-1"));
  applyToolLifecycle(a, searchDone("turn-1", [NODE_A, NODE_B, NODE_C]));
  check("search completion activates each hit on this turn", () => {
    return (
      activationLevelOf(a, NODE_A) === "turn" &&
      activationLevelOf(a, NODE_B) === "turn" &&
      activationLevelOf(a, NODE_C) === "turn"
    );
  });
}

// 6) Historical: a node previously seen in another session is `historical`
//    when there's no live / completed turn ref.
{
  const a = emptyActivation();
  a.historical = new Set([NODE_A]);
  check("historical-only id resolves to 'historical'", () => activationLevelOf(a, NODE_A) === "historical");
}

// 7) Live trumps historical: if the agent is using a wiki right now
//    AND it's in the historical set, "live" wins.
{
  const a = emptyActivation();
  a.historical = new Set([NODE_A]);
  applyToolLifecycle(a, readStart("turn-1", NODE_A));
  check("live overrides historical", () => activationLevelOf(a, NODE_A) === "live");
}

// 8) Error phase still counts as a use (the agent asked for the wiki) —
//    error frames behave like completion for the brain's purposes.
{
  const a = emptyActivation();
  applyToolLifecycle(a, readStart("turn-1", NODE_A));
  applyToolLifecycle(a, ev("turn-1", "corpus_read", "error", { id: NODE_A }));
  check("error phase still moves 'live' → 'turn'", () => activationLevelOf(a, NODE_A) === "turn");
}

// 9) Non-corpus tools are ignored — only corpus_search / corpus_read
//    feed the activation map.
{
  const a = emptyActivation();
  applyToolLifecycle(a, {
    seq: 1,
    kind: "tool_lifecycle",
    payload: {
      turn_id: "turn-1",
      phase: "completion",
      data: { tool: "web_fetch", display_payload: { url: "https://x" } },
    },
    created_at: "2026-05-24",
  });
  check("non-corpus tool does not populate byNode", () => a.byNode.size === 0);
}

// 10) corpusIdsFromEvent: search with empty hits → empty array, no crash.
{
  const ids = corpusIdsFromEvent(searchDone("turn-1", []));
  check("empty search hits yields no ids", () => Array.isArray(ids) && ids.length === 0);
}

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
