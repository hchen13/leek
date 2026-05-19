// Shared types for the L.E.E.K workbench.
//
// The frontend consumes the M1.9 event contract (see the gateway's
// `agent/events.rs`). Every event names the workbench `surface` that owns
// it; the canvas process events (`note_trace` / `tool_lifecycle` /
// `search_lifecycle`) share one `CanvasArtifact` envelope.

/** A persisted event row — same shape from the SSE stream and the history
 *  endpoint: `{ seq, kind, payload, created_at }`. `payload` carries the
 *  event body plus its stamped `surface`. */
export type EventRow = {
  seq: number;
  kind: string;
  payload: Record<string, unknown>;
  created_at: string;
};

export type Session = {
  id: string;
  title: string | null;
  created_at: string;
  last_active_at: string;
};

export type Message = {
  seq: number;
  role: string;
  content: string;
  created_at: string;
};

/** Which workbench surface consumes an event (gateway `Surface`). The
 *  surface — never the kind — decides the panel an event lands in. */
export type Surface = "chat" | "canvas" | "right_rail" | "lifecycle";

/** Lifecycle phase of a canvas artifact frame. */
export type Phase = "start" | "completion" | "error";

/** A `url_citation` source on a provider-search card (REQUIREMENTS §4.3). */
export type Source = { url: string; host: string | null; title: string | null };

/** A canvas process card, accumulated from one or more `CanvasArtifact`
 *  frames sharing an `artifactId`. Notes are instantaneous; tool / search
 *  artifacts move `start` → `completion` / `error`. */
export type Artifact = {
  artifactId: string;
  canvasIdentity: string;
  kind: "note" | "tool" | "search";
  iteration: number;
  phase: Phase;
  // note
  text?: string;
  // tool
  tool?: string;
  displayName?: string;
  cardKind?: string;
  summary?: string;
  args?: unknown;
  displayPayload?: Record<string, unknown>;
  debugPayload?: Record<string, unknown>;
  // search
  query?: string | null;
  sources?: Source[];
};

export type TurnMetrics = {
  iterationCount: number;
  toolCallCount: number;
  toolErrorCount: number;
  wallClockMs: number;
};

/** A turn's canvas section plus its lifecycle status. Built from the
 *  `canvas` and `lifecycle` events that carry the turn's `turn_id`. */
export type Turn = {
  turnId: string;
  artifacts: Artifact[]; // insertion order = chronological
  status: "running" | "done";
  stopReason: string | null;
  /** Assistant message seq, from `assistant_done` — links turn ↔ chat. */
  messageSeq: number | null;
  compactions: number;
  metrics: TurnMetrics | null;
  error: string | null;
};

export type PlanStep = {
  step: string;
  status: "pending" | "in_progress" | "completed";
};

/** The right-rail Plan / TODO (REQUIREMENTS §2.6). Replaced wholesale by
 *  each `plan_updated` event. */
export type Plan = {
  turnId: string;
  steps: PlanStep[];
  explanation: string | null;
};
