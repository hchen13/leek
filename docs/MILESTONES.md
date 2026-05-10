# L.E.E.K Rebuild Roadmap

> **Source of truth for the rebuild-clean branch's goals and ordering.**
>
> Companion to `docs/ARCHITECTURE.md`. ARCHITECTURE tells you the
> end-state shape; this file tells you the order of arrival.
>
> Update on every milestone completion. The conversational context
> that produces these milestones will compact many times before M4
> lands — this file exists so that after compaction, both the human
> and any LLM continuing the work can re-ground without losing
> decisions.

---

## How to use this file

- Each milestone has **Scope**, **Sub-commits** (planned), **Design
  decisions (locked)**, and **Open questions**.
- "Locked" decisions came from cross-repo investigation (codex-rs,
  claude-code, hermes-agent, openclaw) or explicit user/agent
  dialog. Don't second-guess them silently — raise it explicitly.
- "Open questions" are deferred-but-tracked. Resolve before the
  relevant sub-commit; don't let them rot.
- Cross-cutting principles live in `ARCHITECTURE.md §12`.
- Decision log at the very end records *why* the locked decisions
  were locked.

---

## Status banner (2026-05-11)

The `rebuild` branch reached M1-done + Phase 0g cleanup, then was
diagnosed as carrying too much deterministic-systems scaffolding
(routing layer, deliverable taxonomy, plan_guard) to be salvageable
within the budget. **Decision: delete the agent backend, restart on
`rebuild-clean`.** Frontend code stays. Locked design decisions
survive (they were research-backed, independent of the code that
implemented them poorly). See "Decision log" → 2026-05-11 entry.

Previous milestone marks (M1 DONE etc.) are intentionally reset —
the *code* that implemented them is being deleted. The *principles*
they validated remain locked.

---

## M0 — Clean skeleton

### Goal

Stand up the smallest end-to-end vertical slice that proves the
plumbing: a user can log in, open a session, send a message, get a
response back through SSE. **No agent loop yet. No tools yet. No
LLM call yet.** The "response" is a server-side echo. Front-to-back
wiring is what we're proving.

### Scope

- Branch surgery: delete the old agent / routing / deliverable /
  charter / decision / plan / portfolio / holdings / subagents /
  task_metrics / tool_runs / compaction code paths. See
  `ARCHITECTURE.md §10` for the literal delete list.
- Migration consolidation: collapse 10 old migrations into a single
  `0001_initial.sql` covering the M0 schema (`users`, `sessions`,
  `messages`, `user_settings`).
- Frontend reshape: delete cards / panels tied to removed entities
  (charter, decision draft, portfolio, deliverable artifact, task
  status). Keep the chat composer, message list, SSE wiring, corpus
  viewer, plan view (plan view is dormant until M1, that's fine).
- Verify: send a message, see it persist, see an SSE echo response
  arrive. No agent, no LLM.

### Sub-commits (planned)

| # | Title | Scope |
|---|-------|-------|
| M0.1 | Delete old backend | Remove all `crates/gateway/src/agent/`, the deleted vault modules, the deleted API routes |
| M0.2 | Migration reset | Collapse migrations 0001–0010 into a single new `0001_initial.sql` |
| M0.3 | Echo loop | Server-side stub: POST message → SSE event echo response → DB persisted |
| M0.4 | Frontend reshape | Remove panels for deleted entities; verify end-to-end echo works |

### Design decisions (locked)

- **Single migration file at M0**, not preserved-history. We are
  not preserving the old vault format. New users on the new schema.
- **Echo response, not LLM call, at M0**. Separating plumbing from
  agent logic keeps M0 small and lets M1 own the model integration
  end-to-end.
- **No `tasks` table.** Conversations are sessions of messages.
  See `ARCHITECTURE.md §6`.

### Open questions

- Existing vault DBs on the developer's machine — drop them, or
  write a one-shot migrator? Default: drop (we have no production
  users yet).

---

## M1 — Agent Loop MVP: safety nets

### Goal

Same as the old M1 (which validated this shape): make the main
agent loop work end-to-end against codex OAuth, with all harness
safety nets in place. Cap things, but make most caps opt-in
(mirroring codex's "trust the provider" philosophy).

### Scope

- Main agent loop: codex OAuth → Responses API → SSE streaming back
  to client
- Tool registry plumbing (1–2 trivial tools: `web_fetch`, maybe
  `echo` for testing)
- Safety nets (see Locked decisions table below)
- `turn_metrics` vault table + write hook
- Per-turn observability event (`turn_metrics_recorded` SSE)

### Sub-commits (planned)

| # | Title | Default |
|---|-------|---------|
| M1.1 | turn_metrics table + GuardConfig scaffold | — |
| M1.2 | Codex OAuth call + bare loop (no guards yet, just iterates) | — |
| M1.3 | Idle timeout | 90 s, on |
| M1.4 | Wall-clock ceiling + staged soft-prompts | 30 min, on |
| M1.5 | Iteration cap | None, opt-in |
| M1.6 | Cost cap + per-model price table | None, opt-in |
| M1.7 | Doom-loop detector + first_triggered_guard wiring | N=3, on |
| M1.8 | Auto-compaction | 90%, on |

### Design decisions (locked — carried over from prior rebuild)

These were validated by cross-repo research on the prior `rebuild`
branch. The validation survives the code reset.

- **Iteration cap is opt-in, default `None`** — neither codex nor
  claude-code enforces one. Codex explicitly trusts auto-compaction.
  Old leek had hardcoded `MAX_TOOL_TURNS=24` which was too tight
  (real complex A-share research routinely hits 20+ iterations
  legitimately). openclaw has `[32, 160]`. We side with codex / CC.

- **Cost cap is opt-in, default `None`** — codex doesn't track
  cost. leek wires the mechanism for power users / production but
  defaults off.

- **Wall-clock ceiling 30 min, on by default** — claude-code
  historically had a 5-min hardcoded request timeout; their
  CHANGELOG explicitly says they removed it as a bug. 30 min is a
  true edge-case ceiling, not the active guard.

- **Idle timeout 90 s, on by default** — mirrors claude-code's
  `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`. Primary responsiveness
  guard. openclaw has analogous `turnCompletionIdleTimeoutMs=60s`.

- **Doom-loop detector N=3, on by default** — leek-original. None
  of codex / CC / hermes / openclaw has equivalent. Triggers on
  identical `(tool_name, args)` ≥ 3 consecutive times.

- **Auto-compaction at 90%** — mirrors codex's hardcoded
  `(context_window * 9) / 10`.

- **Soft-prompt time hints are leek-original** — staged 10/5/2/1
  min thresholds, injected per-LLM-block (not per-turn). > 10 min
  remaining injects nothing — most turns never see this guard.

  Staged copy:
  - `≤ 60s`: "wrap up immediately with what you have, no new tool calls"
  - `61–120s`: "write a concise conclusion now; finish any pending tool call but do not start new ones"
  - `121–300s`: "start framing your final answer; defer any non-essential investigation"
  - `301–600s`: "consider scoping down further analysis; prefer breadth-first if multiple branches remain"

- **`ChatRequest.max_output_tokens` not sent** — codex-rs's
  `ResponsesApiRequest` struct has no field. Trust provider's
  per-model default.

- **No LLM provider abstraction** — only one path (codex OAuth).
  Re-abstract when a second concrete provider arrives. Saves a
  trait hierarchy maintenance cost and a misleading "swap providers
  easily" claim.

### Naming convention (locked)

- *turn*: one user prompt → one final assistant message
- *iteration*: one LLM call within a turn
- `turn_metrics` table is keyed by turn, one row per turn
- The loop variable internally is `iteration_count`, not `turn`

### Open questions

- Per-model wall-clock default? (Currently global 30 min. Reasoning
  models might warrant longer.) Defer until first real complaint.
- Cost cap surfaces multi-tier pricing (input vs cached input vs
  reasoning vs output) — concrete schema in M1.6 commit message.

---

## M2 — Corpus + Mandate

### Goal

Bring the two pieces of content that differentiate leek from a
generic agent: the investment **corpus** and the per-user
**mandate**. Both surface in the main agent's system prompt and via
tools.

### Scope

- Corpus loader: markdown files under a root, lexical (BM25) search
- Tools: `corpus_search(query)`, `corpus_read(id)`
- Default corpus injection in main agent system prompt (target
  < 800 tokens)
- User mandate: `user_settings.mandate_text`, injected into system
  prompt
- Mandate collection UX (onboarding flow) + edit UX (settings page
  or in-chat slash command — see Open questions)

### Sub-commits (planned)

| # | Title |
|---|-------|
| M2.1 | Corpus loader + BM25 index, in-memory |
| M2.2 | `corpus_search` + `corpus_read` tools |
| M2.3 | Default-injection of curated corpus snippets in system prompt |
| M2.4 | `user_settings.mandate_text` + verbatim injection |
| M2.5 | Mandate edit UX (settings) |
| M2.6 | Mandate collection onboarding (first-session flow) |

### Design decisions (locked)

- **Lexical first, embeddings later.** Embeddings cost setup and
  recompute on corpus edits. Defer until lexical hits a measurable
  recall floor.
- **Corpus is git-versioned for now.** Authoring via editing
  markdown files. In-app editor is a later affordance.
- **Mandate is a single markdown blob, not structured fields.**
  We don't know which fields matter until users use leek a lot.
  Free-form text → system prompt → see what the model does with it
  → structure later if obvious patterns emerge.

### Open questions

- Mandate length cap — what happens when it grows past 2K tokens?
  Hard cap, or summarization on save?
- Mandate edit UX surface: in-chat slash command vs settings page
  vs both?
- Corpus update reload — startup-time load is fine for v0, but at
  what point do we want hot reload?

---

## M2.5 — Skill / Hook / Plugin

### Goal

Bring skill / hook / plugin to first-class, mirroring Claude Code's
mature implementation. M2.5 is **not** the place to invent — copy
the conventions, adapt minimally.

### Scope

**Skill**:
- Discovery from bundled (`harness/skills/`), user dir
  (`~/.leek/skills/`), project dir (`<project>/.leek/skills/`)
- Frontmatter: `name`, `description`, optional `allowed_tools`,
  `paths`, `disable-model-invocation`, `model`
- `use_skill(name)` tool for lazy body load
- System prompt skill index (description-only, one bullet per
  skill)
- Skill→tool gating: a tool call inside a skill body restricted to
  the skill's `allowed_tools`
- Hot reload via `notify` crate

**Hook**:
- Match CC's event surface: `PreToolUse`, `PostToolUse`, `Stop`,
  `SubagentStop`, `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
  `PreCompact`, `Notification`
- Hook execution: shell command (capture stdout / exit code per CC
  contract)
- Hook timeout (CC has per-hook `timeout` field, 5–60s typical)
- Block / continue semantics

**Plugin**:
- Bundle of skills + hooks + commands distributed as a unit
- Manifest format mirroring CC plugins
- Local install only in v0

### Open questions

- Where does mandate machinery live in this model? Likely becomes
  a default-installed skill (`harness/skills/mandate/`) plus a
  `SessionStart` hook that injects mandate-text into the system
  prompt. Decided at M2.5 commit time.
- Plugin sandboxing — out of scope for first cut; trust local
  install only.
- Skill model override semantics when paired with codex OAuth (CC
  routes per-skill model; codex pro is one model).

---

## M2.7 — Subagent

### Goal

A generic mechanism for spawning a child agent loop with its own
context window, system prompt, and tool subset. The investment
domain genuinely benefits from this (parallel multi-ticker scans,
corpus-expert delegation, planner separation). See `ARCHITECTURE.md
§4.2-4.3` for the topology rationale.

### Scope

- A `task` tool (CC convention) the main agent calls
- Subagent runs in its own loop with: its own system prompt (often
  skill-derived), its own tool registry subset, its own message
  history
- **Subagent's loop reuses _all_ M1 guards** (cost cap / wall-clock
  / idle / iteration / doom-loop / turn_metrics)
- Result returned to parent as a single text block (not streamed,
  initially)
- Skill-driven persona binding: `task(skill="corpus-expert",
  input="...")` loads the skill body as the subagent's system prompt
  + restricts tools to skill's `allowed_tools`
- Nesting: subagents can spawn subagents, depth limit 2 (main →
  child → grandchild stops there)

### Sub-commits (planned)

| # | Title |
|---|-------|
| M2.7.1 | `task` tool + subagent loop spawn |
| M2.7.2 | Skill-driven persona binding |
| M2.7.3 | Depth limit + per-subagent turn_metrics rows (parent_turn_id linkage) |
| M2.7.4 | First three subagent skills: `corpus-expert`, `market-data-fetcher`, `planner` |

### Design decisions (locked)

- **Tool name: `task`** — CC convention, no reason to diverge.
- **One-shot result (batch), not streaming, in v0** — simpler;
  upgrade to streaming later if UX demands it.
- **Subagent vault scope: own turn within parent session** — no
  separate session entity. `turn_metrics.parent_turn_id` carries
  the linkage.
- **Depth limit 2 by default** — track via `turn_metrics.depth`.

### Dependencies

- **Requires M1** — safety nets must work inside subagent loops too.
- **Requires M2.5 skill** — persona binding via skill is the
  primary use case.
- **Blocks M3** — A-share task shapes ("full-market scan", "deep
  stock review parallel branches") want this.

### Open questions

- Event streaming: stream subagent events to parent in real time
  vs batch on completion. Default batch; reconsider when first
  user-visible latency complaint arrives.
- Subagent mandate visibility — see `ARCHITECTURE.md §7` open
  questions.

---

## M3 — A-share MVP

### Goal

5–7 core A-share tools + 3 task shapes (quick scan, deep review,
comparison). The tools work, the prompts work, the task shapes are
repeatable.

### Scope (high-level — refine when M3 starts)

Tools (re-introduced from the deleted `agent/tools/` set, but
reviewed under the new naming-neutrality and harness-fit lenses):

- `market_quote` — snapshot quotes
- `get_candlesticks` — OHLCV across markets
- `get_financials` — income / balance / cashflow / ratios
- `get_company_info` — profile + latest indicators
- `get_capital_flow` — moneyflow + northbound

Task shapes — proven end-to-end with eval cases:

1. **Quick scan** — "Is X tradable / interesting right now?" One
   subagent (`market-data-fetcher`) gathers data; main agent
   synthesizes. < 2 min wall-clock.
2. **Deep review** — Full stock review. Multiple subagents in
   parallel (data fetch + corpus expert + planner). 5–15 min wall-
   clock typical.
3. **Comparison** — N tickers. N parallel `market-data-fetcher`
   subagents, main agent synthesizes.

### Open questions

- Which 5–7 tools is the right initial set? Listed above is a
  starting hypothesis. Validate against the first 10 real research
  questions.
- Eval set: where do the test queries come from? Likely from the
  user's own research history.

---

## M4 — A-share complete

### Goal (placeholder)

Production-ready A-share research vertical: every common research
question shape covered, retention of conclusions across sessions,
observability dashboards.

[Detailed scope deferred until M3 lands and we see what's missing.]

---

## Decision log (chronological)

### 2026-05-09 — rebuild branch direction
- Tear out (on the now-deleted `rebuild` branch): critic / 4-persona
  subagent / decision_draft pipeline / budget_finalization.
- Keep: 4 panels (chat / canvas / corpus / plan).
- Approach: codex-style conventions where possible.

### 2026-05-09 — soft-prompt + hard ceiling for time
- Wall-clock has both a soft prompt (block-level injection at
  10/5/2/1 min remaining) and a hard ceiling (30 min cancel).
- Soft is leek-original; hard is conservative.
- Cross-repo investigation: codex (no), claude-code (no, removed as
  bug), hermes-agent (no), openclaw (idle-only).

### 2026-05-09 — opt-in vs default-on for guards
- Default-on: idle timeout, wall-clock, doom-loop, auto-compaction,
  observability (`turn_metrics`).
- Opt-in: iteration cap, cost cap.
- Mirroring codex except where leek-original guards (doom-loop,
  soft time hints) make sense to ship on by default.
- Cross-repo investigation: codex (no iteration cap, no cost cap,
  90% auto-compact, no `max_output_tokens`), claude-code (no
  iteration cap, no per-call max_tokens default), openclaw
  (iteration cap [32, 160] scaled, idle timeout 60s, has retry
  cap), hermes-agent (no per-turn caps at all).

### 2026-05-09 — subagent added as M2.7
- Was missing from initial roadmap. Added between skill/hook/plugin
  (M2.5) and A-share MVP (M3) because subagent depends on skill
  machinery (persona binding) and unblocks A-share parallel task
  shapes.

### 2026-05-09 — `max_output_tokens` not sent
- codex-rs's `ResponsesApiRequest` struct has no field; CC and
  others trust provider per-model defaults. leek matches.

### 2026-05-11 — rebuild-clean reset
- Decided to delete the `rebuild` branch's agent backend and restart
  on `rebuild-clean`. Diagnosis: too much deterministic-systems
  scaffolding (routing layer, deliverable taxonomy, plan_guard,
  task entity) accumulated through Phase 0a–0g + M1; each "rescue"
  edit during M1 QA produced more architectural entanglement
  rather than less.
- What survives the reset: cross-cutting principles, locked design
  decisions (idle / wall-clock / doom-loop / auto-compact / etc.),
  the milestone ordering (M0 added; M1–M4 retained).
- What's deleted (literal list): see `ARCHITECTURE.md §10`.
- What's added: `M0 — Clean skeleton` (explicit branch-surgery
  milestone before re-implementing M1).

### 2026-05-11 — no LLM provider abstraction
- Today there is one path: codex pro OAuth → Responses API. The
  user has no third-party API keys; there's no second concrete
  provider to design against.
- The prior `LlmProvider` trait was speculative. Removed in
  rebuild-clean. Re-abstract when a real second provider arrives.

### 2026-05-11 — subagent in the architecture from M0, sub-mechanism in M2.7
- ARCHITECTURE.md describes subagents as part of the end-state
  agent topology (§4.2). MILESTONES.md still implements the
  *mechanism* in M2.7 because: (a) M1 doesn't need subagents to
  prove the loop works, (b) M2.7 properly depends on M2.5's skill
  machinery for persona binding. The architecture is multi-agent
  conceptually from day one even though the spawn mechanism arrives
  at M2.7.
