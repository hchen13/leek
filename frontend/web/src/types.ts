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
 *  artifacts move `start` → `completion` / `error`. Subagent cards
 *  (M2.7) collect every event the spawned subagent emits, plus the
 *  spawn / completion roll-up.
 *
 *  Search-card variants share the `kind: "search"` artifact but render
 *  differently per `actionType`: `search` (a query result list),
 *  `open_page` (one fetched page + snippet), `find_in_page` (matches
 *  inside a page), or an unknown activity. The variant-specific fields
 *  are nullable — only the ones for the current `actionType` are set. */
export type Artifact = {
  artifactId: string;
  canvasIdentity: string;
  kind: "note" | "tool" | "search" | "subagent" | "codex_duplicate_warning";
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
  // subagent (M2.7)
  agentName?: string;
  subagentTurnId?: string;
  depth?: number;
  inputPreview?: string;
  resultPreview?: string;
  stopReason?: string;
  iterationCount?: number;
  toolCallCount?: number;
  toolErrorCount?: number;
  costUsd?: number;
  wallClockMs?: number;
  sourceLayer?: string;
  errorMessage?: string;
  // codex_duplicate_warning (M3.1) — payload is flat (no envelope).
  warningActionType?: string;
  warningUrl?: string;
  warningHost?: string | null;
  warningCount?: number;
  warningThreshold?: number;
  warningAborted?: boolean;
  /** Inner artifacts the subagent emitted — its own tool / note / search
   *  cards (and any nested subagent_card for depth=2). They are routed
   *  here by `parent_turn_id` on the event payload, never flat onto the
   *  parent turn's artifact list. Keyed by inner artifactId so a
   *  start→completion update is a merge, not a duplicate. */
  innerArtifacts?: Artifact[];
};

export type TurnMetrics = {
  iterationCount: number;
  toolCallCount: number;
  toolErrorCount: number;
  wallClockMs: number;
};

/** Cost cap trip details — populated from a `turn_cost_capped` event
 *  (M2.6). The chat surface uses this to render an in-turn warning bar
 *  with the actual cap + spend + iteration count. `null` until the cap
 *  trips for a given turn. */
export type TurnCostCap = {
  capUsd: number;
  actualCostUsd: number;
  iterCount: number;
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
  /** Set when the per-turn cost cap tripped (M2.6). */
  costCap: TurnCostCap | null;
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

// ─── settings (M2.6) ──────────────────────────────────────────────────────
//
// The settings API returns `{ config, effective, config_path }`. `config`
// is the raw `~/.leek/config.json` (every field may be null = "user has not
// set it"); `effective` is what `GuardConfig::resolve` actually applies
// after env vars layer on top, plus an `overridden_by_env` flag per field so
// the UI can show a "⚠ env var override" badge without re-doing the
// resolution.

/** Raw config-file shape — every field optional. Mirrors the Rust
 *  `crate::config::Config`. */
export type SettingsConfig = {
  idle_timeout_secs?: number | null;
  wall_clock_secs?: number | null;
  max_iterations?: number | null;
  cost_cap_usd?: number | null;
  doom_loop_threshold?: number | null;
  auto_compact_threshold?: number | null;
  context_window?: number | null;
  /** M3 vendor token. Optional, masked in UI. */
  tushare_token?: string | null;
  /** M3.1 codex builtin web_search duplicate-URL warn threshold. */
  builtin_url_warn_threshold?: number | null;
  /** M3.1 codex builtin web_search duplicate-URL abort threshold. */
  builtin_url_abort_threshold?: number | null;
};

/** One field's effective value + env-override flag. The value is `null`
 *  when the guard is disabled (e.g. `cost_cap_usd = 0`). For string fields
 *  like tushare_token the value is a string instead of a number. */
export type EffectiveField = {
  value: number | string | null;
  overridden_by_env: boolean;
};

/** Shape returned by `GET /api/v1/settings` (and the `PATCH` response). */
export type SettingsResponse = {
  config: SettingsConfig;
  effective: {
    idle_timeout_secs: EffectiveField;
    wall_clock_secs: EffectiveField;
    max_iterations: EffectiveField;
    cost_cap_usd: EffectiveField;
    doom_loop_threshold: EffectiveField;
    auto_compact_threshold: EffectiveField;
    context_window: EffectiveField;
    /** M3 Tushare token (string field, masked in UI). */
    tushare_token: EffectiveField;
    /** M3.1 codex builtin web_search duplicate-URL warn threshold. */
    builtin_url_warn_threshold: EffectiveField;
    /** M3.1 codex builtin web_search duplicate-URL abort threshold. */
    builtin_url_abort_threshold: EffectiveField;
    web_search: { value: boolean; overridden_by_env: boolean };
  };
  config_path: string;
};
