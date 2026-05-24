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

/** One result on a provider-search card — a title (may fall back to host),
 *  the URL it points to, and the URL's host (parsed gateway-side).
 *  (REQUIREMENTS §4.3, MILESTONES decision 2026-05-20.) */
export type SearchResult = { title: string | null; url: string; host: string | null };

/** A canvas process card, accumulated from one or more `CanvasArtifact`
 *  frames sharing an `artifactId`. Notes are instantaneous; tool / search
 *  artifacts move `start` → `completion` / `error`.
 *
 *  Search-card variants share the `kind: "search"` artifact but render
 *  differently per `actionType`: `search` (a query result list),
 *  `open_page` (one fetched page + snippet), `find_in_page` (matches
 *  inside a page), or an unknown activity. The variant-specific fields
 *  are nullable — only the ones for the current `actionType` are set. */
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
  // search — common
  actionType?: string;
  // search — `search` variant
  query?: string | null;
  results?: SearchResult[];
  resultsTotal?: number;
  // search — `open_page` / `find_in_page` variants
  pageUrl?: string;
  pageHost?: string | null;
  pageTitle?: string | null;
  pageSnippet?: string | null;
  pattern?: string;
  matches?: string[];
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

// ── Corpus Brain (M2.1) ──────────────────────────────────────────────

/** One wiki page in the corpus brain. Mirrors `corpus::GraphNode`. */
export type BrainNode = {
  id: string;
  title: string;
  tier: string;
  type: string;
  tags: string[];
  confidence: string;
  slug: string;
  directory_path: string;
};

/** Shared-tag edge between two wiki pages (`weight = |shared tags|`). */
export type BrainEdge = {
  source: string;
  target: string;
  weight: number;
  shared_tags: string[];
};

/** The full corpus brain graph payload — `/api/v1/corpus/graph`. */
export type BrainGraph = {
  nodes: BrainNode[];
  edges: BrainEdge[];
  stats: {
    node_count: number;
    edge_count: number;
    weight_histogram: Record<string, number>;
  };
};

/** Where a wiki sits in the agent-usage timeline (REQUIREMENTS §2.5):
 *
 *   live        | a corpus tool is calling this id RIGHT NOW
 *   turn        | the current turn finished a call against this id
 *   session     | an earlier turn of this session used this id
 *   historical  | a prior session used this id (persisted)
 */
export type ActivationLevel = "live" | "turn" | "session" | "historical";

/** Per-node activation record. `liveTurns` is what a Start frame adds;
 *  Completion moves the entry from `liveTurns` into `completedTurns`. */
export type NodeActivationRef = {
  liveTurns: Set<string>;
  completedTurns: Set<string>;
};

/** Aggregated activation state owned by the workbench store. */
export type Activation = {
  /** Per-node ref counts — see `NodeActivationRef`. */
  byNode: Map<string, NodeActivationRef>;
  /** The most recent turn the store has seen any activity on. The brain
   *  uses this to distinguish "this turn" from "earlier in the session". */
  currentTurn: string | null;
  /** Node ids loaded from sessionStorage on session open — wikis used in
   *  ANY prior session. The store seeds this and updates it on session
   *  switch / unload. */
  historical: Set<string>;
};
