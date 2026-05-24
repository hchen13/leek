// The workbench store — turns the M1.9 event stream into panel state.
//
// One entry point, `applyEvent`, routes each event by its `payload.surface`
// (REQUIREMENTS §8.2): the surface decides the panel, never a hardcoded
// kind→panel map. Within a surface, the kind selects how the event updates
// state. The store holds canvas turns, the plan, the streaming-reply
// reconciliation state, and (M2.1) the Corpus Brain activation snapshot.
// The chat message list itself stays in `App` (loaded over REST, refreshed
// on `message_created`).

import { createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";

import type {
  Activation,
  Artifact,
  EventRow,
  NodeActivationRef,
  Phase,
  Plan,
  PlanStep,
  Surface,
  Turn,
} from "./types";

export type WorkbenchState = {
  /** Canvas sections, in first-seen (chronological) turn order. */
  turns: Turn[];
  /** Right-rail Plan / TODO, or null when the agent has not made one. */
  plan: Plan | null;
  /** The reply currently streaming — only the latest iteration's text. */
  streaming: { turnId: string; iteration: number; text: string } | null;
  /** `"<turnId>:<iteration>"` keys whose streamed text was a Note Trace,
   *  not the final reply — used to keep notes out of the chat bubble. */
  noted: Record<string, true>;
};

const EMPTY: WorkbenchState = { turns: [], plan: null, streaming: null, noted: {} };

function planStatus(v: unknown): PlanStep["status"] {
  return v === "in_progress" || v === "completed" ? v : "pending";
}

/** The artifact kind for a canvas event, or null if it is not one we draw. */
function canvasKind(eventKind: string): Artifact["kind"] | null {
  if (eventKind === "note_trace") return "note";
  if (eventKind === "tool_lifecycle") return "tool";
  if (eventKind === "search_lifecycle") return "search";
  return null;
}

/** Merge a `CanvasArtifact` frame's `data` into an artifact. Fields absent
 *  from a frame (a start frame has no `display_payload`) are left intact. */
function mergeData(art: Artifact, kind: Artifact["kind"], data: Record<string, unknown>) {
  if (kind === "note") {
    art.text = String(data.text ?? "");
    return;
  }
  if (kind === "tool") {
    if (data.tool != null) art.tool = String(data.tool);
    if (data.display_name != null) art.displayName = String(data.display_name);
    if (data.card_kind != null) art.cardKind = String(data.card_kind);
    if (data.summary != null) art.summary = String(data.summary);
    if (data.arguments !== undefined) art.args = data.arguments;
    if (data.display_payload !== undefined) {
      art.displayPayload = data.display_payload as Record<string, unknown>;
    }
    if (data.debug_payload !== undefined) {
      art.debugPayload = data.debug_payload as Record<string, unknown>;
    }
    return;
  }
  // search — the activity variant is in `action_type`. A Start frame has
  // an empty `data`, so `actionType` may be unset until the Completed frame
  // arrives; the renderer treats unset as a still-loading state.
  if (data.action_type != null) art.actionType = String(data.action_type);

  if (data.action_type === "search") {
    if (data.query !== undefined) {
      art.query = data.query == null ? null : String(data.query);
    }
    if (Array.isArray(data.results)) {
      art.results = data.results.map((r) => {
        const o = (r ?? {}) as Record<string, unknown>;
        return {
          title: o.title == null ? null : String(o.title),
          url: String(o.url ?? ""),
          host: o.host == null ? null : String(o.host),
        };
      });
    }
    if (data.results_total != null) art.resultsTotal = Number(data.results_total);
  } else if (data.action_type === "open_page") {
    if (data.url != null) art.pageUrl = String(data.url);
    if (data.host !== undefined) art.pageHost = data.host == null ? null : String(data.host);
    if (data.title !== undefined) art.pageTitle = data.title == null ? null : String(data.title);
    if (data.snippet !== undefined) {
      art.pageSnippet = data.snippet == null ? null : String(data.snippet);
    }
  } else if (data.action_type === "find_in_page") {
    if (data.url != null) art.pageUrl = String(data.url);
    if (data.host !== undefined) art.pageHost = data.host == null ? null : String(data.host);
    if (data.pattern != null) art.pattern = String(data.pattern);
    if (Array.isArray(data.matches)) {
      art.matches = data.matches.map((m) => String(m ?? ""));
    }
  }
  // Any other `action_type` (Unknown variant) renders by its name with no
  // variant-specific fields; a Start frame (no action_type) carries
  // nothing yet and waits for Completed.
}

/** Pull the wiki ids a `tool_lifecycle` event references — empty when
 *  the tool isn't a corpus tool, or its payload doesn't carry usable ids.
 *  `corpus_read.display_payload.id` is a single string;
 *  `corpus_search.display_payload.hits[].id` is an array.
 *
 *  Exported because the activation state machine is unit-tested in
 *  `tests/activation.test.mjs` against synthetic event payloads. */
export function corpusIdsFromEvent(ev: EventRow): string[] {
  const data = (ev.payload.data ?? {}) as Record<string, unknown>;
  const tool = String(data.tool ?? "");
  const dp = (data.display_payload ?? {}) as Record<string, unknown>;
  if (tool === "corpus_read") {
    const id = dp.id;
    return typeof id === "string" && id ? [id] : [];
  }
  if (tool === "corpus_search") {
    const hits = Array.isArray(dp.hits) ? (dp.hits as Array<Record<string, unknown>>) : [];
    return hits.map((h) => String(h.id ?? "")).filter((s) => s.length > 0);
  }
  return [];
}

/** Compute a node's activation level from the snapshot. Exported for
 *  unit tests; the CorpusBrain component carries its own copy because
 *  it also handles the "no activation" case. */
export function activationLevelOf(
  activation: Activation,
  nodeId: string,
): "live" | "turn" | "session" | "historical" | null {
  const ref = activation.byNode.get(nodeId);
  if (!ref) return activation.historical.has(nodeId) ? "historical" : null;
  if (activation.currentTurn && ref.liveTurns.has(activation.currentTurn)) return "live";
  if (activation.currentTurn && ref.completedTurns.has(activation.currentTurn)) return "turn";
  if (ref.completedTurns.size > 0) return "session";
  return activation.historical.has(nodeId) ? "historical" : null;
}

/** Key under which session-historical wiki activations are persisted.
 *  Per-session keying so unrelated sessions don't blur each other's
 *  history. The store seeds the current session's set from `<all priors>`
 *  on construction so a returning user sees their wiki past. */
const HISTORY_STORAGE_KEY = "leek.brain.historical.v1";

function loadHistorical(): Set<string> {
  try {
    const raw = sessionStorage.getItem(HISTORY_STORAGE_KEY);
    if (!raw) return new Set();
    const arr = JSON.parse(raw) as unknown;
    if (!Array.isArray(arr)) return new Set();
    return new Set(arr.filter((v): v is string => typeof v === "string"));
  } catch {
    return new Set();
  }
}

function saveHistorical(s: Set<string>) {
  try {
    sessionStorage.setItem(HISTORY_STORAGE_KEY, JSON.stringify([...s]));
  } catch {
    // sessionStorage may be full / disabled — degrade silently.
  }
}

export function createWorkbench() {
  const [state, setState] = createStore<WorkbenchState>({ ...EMPTY, noted: {}, turns: [] });

  // Activation snapshot. The brain reads via `activation()` and we
  // re-set the whole signal on each mutation (`Map`/`Set` mutations
  // don't fire Solid reactivity). Cheap — at most a few thousand entries.
  const initialActivation: Activation = {
    byNode: new Map(),
    currentTurn: null,
    historical: loadHistorical(),
  };
  const [activation, setActivation] = createSignal<Activation>(initialActivation);

  function updateActivation(mutate: (a: Activation) => void) {
    const next: Activation = {
      byNode: new Map(activation().byNode),
      currentTurn: activation().currentTurn,
      historical: new Set(activation().historical),
    };
    mutate(next);
    setActivation(next);
  }

  /** Roll the current session's used-wiki ids into the persisted
   *  "historical" pool. Called on session switch / unload. */
  function rollOverToHistorical() {
    const a = activation();
    if (a.byNode.size === 0) return;
    const merged = new Set(a.historical);
    for (const id of a.byNode.keys()) merged.add(id);
    saveHistorical(merged);
  }

  /** Clear all state — called when switching sessions. */
  function reset() {
    // The current session's activations roll into history before we drop
    // them — that's how a wiki used in session N appears as "historical"
    // when the user comes back to session M.
    rollOverToHistorical();
    setState({ turns: [], plan: null, streaming: null, noted: {} });
    setActivation({
      byNode: new Map(),
      currentTurn: null,
      historical: loadHistorical(),
    });
  }

  /** Find a turn in the draft, creating it if this is its first event. */
  function ensureTurn(d: WorkbenchState, turnId: string): Turn {
    const found = d.turns.find((t) => t.turnId === turnId);
    if (found) return found;
    d.turns.push({
      turnId,
      artifacts: [],
      status: "running",
      stopReason: null,
      messageSeq: null,
      compactions: 0,
      metrics: null,
      error: null,
    });
    return d.turns[d.turns.length - 1];
  }

  function applyCanvas(ev: EventRow) {
    const kind = canvasKind(ev.kind);
    const turnId = String(ev.payload.turn_id ?? "");
    if (!kind || !turnId) return;
    const artifactId = String(ev.payload.artifact_id ?? "");
    const phase = (ev.payload.phase as Phase) ?? "completion";
    const iteration = Number(ev.payload.iteration ?? 0);
    const canvasIdentity = String(ev.payload.canvas_identity ?? artifactId);
    const data = (ev.payload.data ?? {}) as Record<string, unknown>;

    setState(
      produce((d: WorkbenchState) => {
        const turn = ensureTurn(d, turnId);
        // A note tells the chat bubble that this iteration's streamed text
        // was narration, not the final reply.
        if (kind === "note") d.noted[`${turnId}:${iteration}`] = true;

        const idx = turn.artifacts.findIndex((a) => a.artifactId === artifactId);
        if (idx < 0) {
          const art: Artifact = { artifactId, canvasIdentity, kind, iteration, phase };
          mergeData(art, kind, data);
          turn.artifacts.push(art);
        } else {
          // A later frame of the same artifact (start → completion / error,
          // or the sources-enriched search frame) updates one card.
          const art = turn.artifacts[idx];
          art.phase = phase;
          art.iteration = iteration;
          mergeData(art, kind, data);
        }
      }),
    );

    // Corpus Brain activation (M2.1) — tool_lifecycle for corpus_search /
    // corpus_read advances the per-node state machine. We deliberately
    // also process replayed (history) events: the brain shows session-
    // wide usage, and history replay is the only way that information
    // exists when an old session is re-opened.
    if (ev.kind !== "tool_lifecycle") return;
    const ids = corpusIdsFromEvent(ev);
    if (ids.length === 0) return;
    updateActivation((a) => {
      a.currentTurn = turnId;
      for (const id of ids) {
        const ref: NodeActivationRef = a.byNode.get(id) ?? {
          liveTurns: new Set(),
          completedTurns: new Set(),
        };
        // Clone the inner sets so the previous activation snapshot the
        // brain may still be reading isn't mutated out from under it.
        const next: NodeActivationRef = {
          liveTurns: new Set(ref.liveTurns),
          completedTurns: new Set(ref.completedTurns),
        };
        if (phase === "start") {
          next.liveTurns.add(turnId);
        } else {
          // completion OR error — both end the "live" state. An error
          // still counts as a use of the wiki: the agent asked for it.
          next.liveTurns.delete(turnId);
          next.completedTurns.add(turnId);
        }
        a.byNode.set(id, next);
      }
    });
  }

  function applyChat(ev: EventRow) {
    if (ev.kind === "assistant_delta") {
      const turnId = String(ev.payload.turn_id ?? "");
      const iteration = Number(ev.payload.iteration ?? 0);
      const text = String(ev.payload.text ?? "");
      setState(
        produce((d: WorkbenchState) => {
          if (
            !d.streaming ||
            d.streaming.turnId !== turnId ||
            d.streaming.iteration !== iteration
          ) {
            // A new iteration — the bubble shows only the latest one.
            d.streaming = { turnId, iteration, text };
          } else {
            d.streaming.text += text;
          }
        }),
      );
    } else if (ev.kind === "message_created" && ev.payload.role === "assistant") {
      // The final reply is persisted — drop the optimistic bubble.
      setState("streaming", null);
    }
  }

  function applyRightRail(ev: EventRow) {
    if (ev.kind !== "plan_updated") return;
    const raw = Array.isArray(ev.payload.plan) ? ev.payload.plan : [];
    const steps: PlanStep[] = raw.map((s) => {
      const o = (s ?? {}) as Record<string, unknown>;
      return { step: String(o.step ?? ""), status: planStatus(o.status) };
    });
    setState("plan", {
      turnId: String(ev.payload.turn_id ?? ""),
      steps,
      explanation: ev.payload.explanation == null ? null : String(ev.payload.explanation),
    });
  }

  function applyLifecycle(ev: EventRow) {
    const turnId = String(ev.payload.turn_id ?? "");
    if (!turnId) return;
    // Brain activation: assistant_done settles `currentTurn` to the
    // just-finished turn so the brain's "this turn" highlight follows
    // the user's most recent question even if no tool was called this
    // turn. The next corpus_* event of the next turn will move it on.
    if (ev.kind === "assistant_done" || ev.kind === "turn_metrics_recorded") {
      updateActivation((a) => {
        a.currentTurn = turnId;
      });
    }
    setState(
      produce((d: WorkbenchState) => {
        const turn = ensureTurn(d, turnId);
        if (ev.kind === "assistant_done") {
          turn.status = "done";
          turn.stopReason =
            ev.payload.stop_reason == null ? null : String(ev.payload.stop_reason);
          if (ev.payload.message_seq != null) {
            turn.messageSeq = Number(ev.payload.message_seq);
          }
        } else if (ev.kind === "turn_metrics_recorded") {
          turn.metrics = {
            iterationCount: Number(ev.payload.iteration_count ?? 0),
            toolCallCount: Number(ev.payload.tool_call_count ?? 0),
            toolErrorCount: Number(ev.payload.tool_error_count ?? 0),
            wallClockMs: Number(ev.payload.wall_clock_ms ?? 0),
          };
          if (turn.stopReason == null && ev.payload.stop_reason != null) {
            turn.stopReason = String(ev.payload.stop_reason);
          }
        } else if (ev.kind === "compaction_started") {
          turn.compactions += 1;
        } else if (ev.kind === "error") {
          turn.error = ev.payload.message == null ? null : String(ev.payload.message);
          turn.status = "done";
        }
      }),
    );
  }

  /** Route one event to its panel by `payload.surface`. */
  function applyEvent(ev: EventRow) {
    const surface = ev.payload.surface as Surface | undefined;
    if (surface === "chat") applyChat(ev);
    else if (surface === "canvas") applyCanvas(ev);
    else if (surface === "right_rail") applyRightRail(ev);
    else if (surface === "lifecycle") applyLifecycle(ev);
    // An event with no / unknown surface is ignored — the panel is never
    // guessed from the kind.
  }

  return { state, applyEvent, reset, activation };
}

export type Workbench = ReturnType<typeof createWorkbench>;
