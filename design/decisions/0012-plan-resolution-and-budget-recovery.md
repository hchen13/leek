# 0012 — Plan resolution and budget recovery

**Status:** Proposed (next slice)
**Date:** 2026-05-08

## Context

The current slice (skill detangle + remove auto-seed of plan from skill
headings) exposes the next layer of harness debt:

- `agent_plan_items.status` is `pending / in_progress / completed`. There
  is no first-class way to express "agent decided, with evidence, that this
  item cannot be completed in this turn." The plan guard then has only one
  blunt check: literal `completed`.
- `MAX_TOOL_TURNS` (24) and `plan_guard_exhausted` (3 rewrites) currently
  raise `error` events and call `fail_task`. The user sees a failed task,
  not a substantive answer that names what was attempted, what was found,
  and what is missing. Caps are doing the job of normal completion.
- "What did the agent actually try, and how did each attempt fail?" is
  currently a manual reconstruction from `vault.events` + `vault.tool_runs`.
  No durable per-task ledger exists. The model is implicitly asked to keep
  this in its own working memory each turn, which is exactly the kind of
  state harness work is supposed to externalize.

This note records the resolution model and budget-finalization shape we
will implement in the next slice. It is not a spec — it sets the contract
so the next implementation lands consistently.

## Layered plan model

Lifecycle and resolution are different questions and should be different
columns. Conflating them is what made the current schema brittle.

- **Lifecycle** answers *where in execution this item is*. Three values
  are sufficient: `pending`, `in_progress`, `completed`. `completed` means
  the item is terminal / closed for this turn — it does **not** imply the
  item was successfully done. This stays the state machine the agent walks
  through.
- **Resolution** answers *why this item is closed*. It is set the moment
  lifecycle moves to `completed`. Values:
  - `done` — the item was executed and produced its expected evidence.
  - `blocked` — required input is unavailable and no acceptable proxy
    exists in this turn (tool failure after retries, user forbade the
    source, source returned valid empty, credential gap).
  - `deferred` — work is meaningful but out of this turn's scope; the
    item should be picked up in a follow-up turn / task.
  - `superseded` — a later plan revision replaced this item with a
    better-shaped one; original is closed by reference, not by failure.
  - `satisfied_by_proxy` — the underlying evidence need is met by a
    different signal than originally planned (peer comp instead of direct
    filing, capital flow instead of channel inventory, etc.).
  - `insufficient_evidence` — the work was attempted but the conclusion
    cannot be drawn at the current confidence; the agent must surface the
    gap and lower the confidence of the final answer accordingly.

All six resolutions live under the single `completed` lifecycle: a
`completed` item is one the agent is no longer working on, regardless of
whether the work succeeded (`done` / `satisfied_by_proxy`) or was closed
with explicit reason (`blocked` / `deferred` / `superseded` /
`insufficient_evidence`). All six are auditable closure; none is
abandonment. Only `pending` and `in_progress` items signal that work is
still owed.

We will model this as one new column rather than two. Concretely:

- Keep `status` as the lifecycle (`pending`, `in_progress`, `completed`).
- Add `resolution` (nullable string) populated when lifecycle moves to a
  terminal state — `completed` requires a resolution; `pending` /
  `in_progress` must have `resolution = NULL`.
- Add a small `notes` / `evidence` extension if needed for closure
  context; the existing `evidence` column already covers the success
  path, so this may simply be reused (with a normalization that, for
  non-`done` resolutions, evidence describes *why* the item was closed
  in this state).

We do **not** introduce `blocked` etc. as lifecycle values. Doing so
would conflate two axes and force every guard / UI consumer to fan out
across both meanings.

## Plan guard contract

The plan guard's job: prevent abandonment, allow auditable adaptation.

Restated as a single rule:

> Every plan item must end the turn with a lifecycle of `completed`. A
> `completed` item must carry a `resolution` and supporting evidence.
> The guard must not require every item to be `done`; `blocked`,
> `deferred`, `satisfied_by_proxy`, `insufficient_evidence`, or
> `superseded` are valid closures provided the item has evidence
> describing the closure reason and the final answer reflects the
> impact.

Implementation direction:

- `plan_guard` returns `Some(guard)` only when one or more items are
  still `pending` or `in_progress`.
- A `completed` item without a resolution is treated as malformed and
  re-prompted as an unresolved item.
- The guard's prompt back to the model lists items by their resolution
  state so the model can either supply evidence and close them or do the
  remaining work.

The non-`done` resolutions must be acknowledged in the final deliverable
(this is a discipline-level requirement: a `decision_draft` that closes
half its plan as `insufficient_evidence` must lower its confidence and
state the gap; it cannot present as a confident verdict).

## Budget as recovery boundary

`MAX_TOOL_TURNS` and `MAX_PLAN_GUARD_REWRITES` are safety nets, not the
primary completion mechanism. When they trip, the harness should not emit
a bare `error` event and a failed task. It should give the model exactly
one **finalization turn** with the following constraints:

- No new substantive tool calls (read-only summary tools are permitted
  if needed; calls that would change vault state are off).
- The system message instructs the model to produce a checkpoint answer:
  what was attempted, what was found, what remains missing, how the gap
  affects confidence, the current conservative action boundary, and what
  continuing would specifically try next.
- The plan is finalized: pending / in_progress items are closed with
  `resolution = blocked` (tool / data unavailable) or `deferred` (out of
  budget but tractable) and the agent's evidence note attached.

The user-visible state at budget exhaustion becomes a real (if cautious)
answer, not a failed task. `fail_task` is reserved for genuine failures:
provider error, user abort, validation error in `record_investment_action`.

## Attempt ledger

The ledger should be auto-aggregated from runtime facts the harness already
records, not maintained by the model.

Sources, in order of authority:

- `vault.tool_runs` — every dispatch with name, args, status (success /
  error), error string, duration. This is the spine of the ledger.
- `vault.events` — `tool_call` (status), `web_search_call`, `plan_updated`,
  `clarification_requested`, `agent_narration`. These give per-turn
  context the bare tool_runs row lacks.
- Tool error payloads — when tools standardize their error shape (next
  slice on tool design), the ledger can classify attempts as
  `transient | validation | empty_result | permission | not_useful`
  rather than a free-string error.
- Plan updates — each `replace_current` is a snapshot; diffing
  consecutive snapshots gives the audit trail of why the plan changed.

The aggregation is a derived view, not a new write path. The model can
read a compact projection ("what did I already try for this evidence
need?") on demand instead of carrying it in working memory.

This makes "at budget exhaustion, summarize the attempt ledger" a cheap
read; it also makes the existing `record_investment_action` enforcement
(opposing case, invalidation, mandate check) verifiable from facts rather
than from the model's claims.

## Where the line sits

Deterministic enforcement (kept / extended in next slice):

- Plan guard refusing closure of `pending` / `in_progress` items.
- `record_investment_action` requiring risks / opposing_case /
  invalidation / mandate_check fields.
- Resolution values are a fixed enum in the schema, not free text.
- Budget caps trigger a finalization turn — that *behavior* is enforced.
- Tool error classification is computed from typed error payloads, not
  free-form prose.

Model-driven freedom (preserved):

- Which evidence path resolves an item (direct, proxy, peer comparison).
- When a plan item should be closed `insufficient_evidence` vs. retried.
- Which tool to try first, when to switch, when to give up an avenue.
- The shape of the final answer (subject to deliverable contract).
- Which subagent to delegate to, and what to pass them.

The general rule, restated: deterministic gates at write boundaries
(vault state, decision artifacts, irreversible actions, user-visible
commitments) and at completion (no closure without evidence). Everything
else is the model's call, with state and evidence observable so the
human can audit.

## Out of scope for this note

- Concrete schema migration SQL (will be a separate migration alongside
  the implementation slice).
- UI rendering of resolution states (frontend slice).
- Subagent ledger projection (depends on this ledger landing first).
- Changes to existing security tradeoffs (covered by 0011).

## Implementation order suggestion

1. Resolution column + plan guard update. Smallest blast radius, unblocks
   honest closure of `blocked` / `insufficient_evidence` items.
2. Budget finalization turn. Replace fatal `MAX_TOOL_TURNS` /
   `plan_guard_exhausted` paths with a single recovery boundary and
   user-visible checkpoint answer.
3. Attempt ledger projection. Read-side view over tool_runs + events;
   add a tool that exposes it to the agent (and possibly to subagents)
   so the model can answer "what have I already tried?" cheaply.
4. Tool error typing. Standardize error shapes across tools so the
   ledger's classification is derived, not parsed from prose.
