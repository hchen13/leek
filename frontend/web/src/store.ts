// The workbench store — turns the M1.9 event stream into panel state.
//
// One entry point, `applyEvent`, routes each event by its `payload.surface`
// (REQUIREMENTS §8.2): the surface decides the panel, never a hardcoded
// kind→panel map. Within a surface, the kind selects how the event updates
// state. The store holds canvas turns, the plan, and the streaming-reply
// reconciliation state; the chat message list itself stays in `App` (loaded
// over REST, refreshed on `message_created`).

import { createStore, produce } from "solid-js/store";

import type { Artifact, EventRow, Phase, Plan, PlanStep, Surface, Turn } from "./types";

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

export function createWorkbench() {
  const [state, setState] = createStore<WorkbenchState>({ ...EMPTY, noted: {}, turns: [] });

  /** Clear all state — called when switching sessions. */
  function reset() {
    setState({ turns: [], plan: null, streaming: null, noted: {} });
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

  return { state, applyEvent, reset };
}

export type Workbench = ReturnType<typeof createWorkbench>;
