# L.E.E.K Rebuild Roadmap

> **Source of truth for the rebuild branch's goals and ordering.**
>
> Update on every milestone completion. The conversational context that
> produces these milestones will compact many times before Milestone 4
> lands — this file exists so that after compaction, both the human and
> any LLM continuing the work can re-ground without losing decisions.

## How to use this file

- Each milestone has **Scope**, **Sub-commits**, **Design decisions
  (locked)**, and **Open questions**.
- "Locked" decisions came from explicit research (cross-repo
  investigation: codex-rs, claude-code, hermes-agent, openclaw) or
  explicit user/agent dialog. Don't second-guess them silently — if
  you want to revisit a locked decision, raise it explicitly.
- "Open questions" are deferred-but-tracked. Resolve before the
  relevant sub-commit; don't let them rot.
- Cross-cutting principles (apply to every milestone) are at the
  bottom. They are mandatory.
- Decision log at the very end records *why* the locked decisions
  were locked, including which other harnesses were checked.

---

## Phase 0 — Rebuild simplification [DONE]

Atomic 6-commit collapse from the over-engineered prototype. End state:
user prompt → minimal tools → answer.

| Commit | Range |
|---|---|
| 0a | backend cuts: critic / decision_draft pipeline / 4-persona subagent / use_skill (later restored in 0d) / budget_finalization |
| 0b | frontend cuts: corresponding renderers (DecisionDraftCard, SubagentArtifact, etc.) |
| 0c | routing prompt + system-prompt framing trimmed |
| 0d | skill mechanism corrected — frontmatter progressive disclosure (use_skill restored as the lazy-load tool) |
| 0e | tool naming neutrality — no upstream provider names in LLM-visible strings |
| 0f | drop dead `ChatRequest.max_output_tokens` field — align with codex (trust provider default) |

### Tools surviving Phase 0 (11 total)
- **generic**: `ask_user_question`, `web_fetch`, `update_plan`, `use_skill`
- **corpus**: `corpus_search`, `corpus_read`
- **market**: `market_quote`, `get_candlesticks`
- **A-share**: `get_financials`, `get_company_info`, `get_capital_flow`

---

## Milestone 1 — Agent Loop MVP: safety nets [DONE 2026-05-10]

### Goal

Make the agent loop survive adversarial / pathological turns — without
sacrificing complex-task viability. Cap things, but make most caps
opt-in (mirroring codex's "trust the provider" philosophy). Build all
guard mechanisms so they're available; ship them on or off by default
according to whether they're well-understood guards or aggressive ones.

### Sub-commits (all landed)

| #    | Commit    | Content                                                                              | Default          |
|------|-----------|--------------------------------------------------------------------------------------|------------------|
| M1.1 | `e4d8401` | `task_metrics` vault table + write hook + `LlmTuning` extended with all guard fields | —                |
| M1.2 | `4d2d17a` | Idle timeout: GuardConfig-driven + per-task hit budget (3 hits → idle_timeout)       | 90s, on          |
| M1.3 | `ceb3716` | Wall-clock ceiling + 10/5/2/1 min staged soft-prompts                                | 30 min, on       |
| M1.4 | `2b7a232` | Iteration cap (opt-in) + rename `turn` → `iteration_count` (SSE dual-key)            | None (off)       |
| M1.5 | `3f0f46d` | Cost cap USD/turn (opt-in) + per-model price table (`llm/pricing.rs`)                | None (off)       |
| M1.6 | `59aacf1` | Doom-loop detector + `task_metrics.first_triggered_guard` wired at all 5 trigger sites | N=3, on        |
| M1.7 | `ef8e5f9` | Auto-compaction parity with codex (95% → 90% via `LlmTuning.guards.auto_compact_threshold`) | 90%, on    |

### Tests
- 140 unit tests pass (was 115 pre-M1, +25 from M1.1-M1.6)
- cargo build clean, no dead-code warnings
- Frontend tsc --noEmit clean (M1.4 SSE field rename uses dual-key fallback)

### Known limitation (deferred)
- The early `return Err(e)` path after exhausted provider retries
  (~`agent/mod.rs:520`) doesn't write a `task_metrics` row. Documented
  in M1.1 / M1.2 / M1.6 commit messages. Folded into the M1 QA cycle
  if it's exercised in E2E.

### Post-M1 QA cycle (in progress)
- 5 rounds of: deep code review via opus max-thinking subagent → fix
  issues → ≥ 3 E2E cases serially via opus xhigh-thinking subagents
  → fix test issues. Then milestone wrap-up.

### Design decisions (locked)

- **Iteration cap is opt-in, default `None`** — neither codex nor
  claude-code enforces one in their core agent loop. Codex explicitly
  trusts auto-compaction. Old leek had hardcoded `MAX_TOOL_TURNS=24`
  which was too tight (real complex A-share research routinely hits
  20+ iterations legitimately). openclaw has `[32, 160]` scaled by
  auth-profile count — more conservative than where leek lands. We
  side with codex / CC.

- **Cost cap is opt-in, default `None`** — codex doesn't track cost.
  leek wires the mechanism for power users / production but defaults
  off.

- **Wall-clock ceiling 30 min, on by default** — claude-code
  historically had a 5-min hardcoded request timeout; their CHANGELOG
  explicitly says they removed it as a bug ("aborted slow backends
  regardless of `API_TIMEOUT_MS`"). 30 min is a true edge-case
  ceiling, not the active guard.

- **Idle timeout 90s, on by default** — mirrors claude-code's
  `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`. This is the *primary*
  responsiveness guard. openclaw has analogous
  `turnCompletionIdleTimeoutMs=60s`.

- **Doom-loop detector N=3, on by default** — leek-original. None of
  codex / CC / hermes / openclaw has equivalent. Triggers on
  identical `(tool_name, args)` ≥ 3 consecutive times.

- **Auto-compaction at 90%** — mirrors codex's hardcoded
  `(context_window * 9) / 10`. leek currently sits at 95% which is
  too tight (less headroom for the compaction LLM call itself).

- **Soft-prompt time hints are leek-original** — no harness studied
  has them. Implementation: per-LLM-block (not per-turn) injection
  of an ephemeral developer message based on remaining time, using
  staged thresholds (10/5/2/1 min). > 10 min remaining injects
  nothing — most turns never see this guard.

  Staged copy:
  - `≤ 60s`: "wrap up immediately with what you have, no new tool calls"
  - `61–120s`: "write a concise conclusion now; finish any pending tool call but do not start new ones"
  - `121–300s`: "start framing your final answer; defer any non-essential investigation"
  - `301–600s`: "consider scoping down further analysis; prefer breadth-first if multiple branches remain"

- **`ChatRequest.max_output_tokens` not sent** — codex-rs's
  `ResponsesApiRequest` struct has no field. Trust the provider's
  per-model default. (Phase-0f cleanup confirmed leek's effective
  behavior was already this — the field was carried but discarded
  at serialization.)

### Naming fix in this milestone

leek's loop variable currently called `turn` is actually an
**iteration counter** within a single user-facing turn. M1.4 renames
everywhere:

- internal: `turn` → `iteration_count` / `current_iteration`
- constant: `MAX_TOOL_TURNS` (deleted) → `LlmTuning.max_iterations`
- SSE / metric field: `turn_count` → `iteration_count`

User-facing "turn" (user prompt → final assistant message) keeps its
name — it's a different concept.

### Open questions

- Per-model wall-clock default? (Currently global 30 min. Reasoning
  models might warrant longer.) Defer until first real complaint.
- Cost cap surfaces multi-tier pricing (input vs cached input vs
  reasoning vs output) — handle in M1.5 schema design when we cross
  that bridge.

---

## Milestone 2 — Corpus MVP

### Goal (placeholder, expand when M1 lands)
Make `corpus_search` / `corpus_read` first-class. Today the corpus is
a static embedded set; M2 makes it a real authored knowledge layer —
versioned content, user-editable, observable, with proper retrieval
signals.

### Open questions
- Authored content management UX (edit in place? GitOps? both?)
- Embedding vs lexical retrieval (currently lexical-only)
- Tier system semantics — `principles` / `wiki` / etc. — formalize

[Detailed scope deferred until M1 lands.]

---

## Milestone 2.5 — Skill / Hook / Plugin (Claude Code conventions)

### Goal
Bring skill / hook / plugin to first-class, mirroring Claude Code's
mature implementation. M2.5 is **not** the place to invent — copy the
conventions, adapt minimally.

### Already partially in place (Phase 0d)
- `harness/skills/<name>/SKILL.md` discovery
- YAML frontmatter (`name`, `description`)
- `use_skill(name)` tool for lazy body load
- System-prompt skill index (description-only)

### Outstanding for M2.5

**Skill**:
- Discovery from user dir `~/.leek/skills/` and project dir
  `<project>/.leek/skills/` (currently bundled-only)
- Hot reload via `notify` crate (skill author edits SKILL.md → next
  turn picks up)
- Frontmatter fields: `allowed_tools`, `paths`,
  `disable-model-invocation`, `model` (per-skill model override)
- Skill→tool gating: a tool call inside a skill body restricted to
  the skill's `allowed_tools`

**Hook**:
- Match Claude Code event surface: `PreToolUse`, `PostToolUse`,
  `Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`,
  `UserPromptSubmit`, `PreCompact`, `Notification`
- Hook execution: shell command (capture stdout / exit code per the
  CC contract)
- Hook timeout (CC has per-hook `timeout` field, 5–60s typical)
- Block / continue semantics

**Plugin**:
- Bundle of skills + hooks + commands distributed as a unit
- Manifest format mirroring CC plugins
- Local install + (much later) remote install

### Open questions
- Where does `mandate.md` live in the new model? It's currently a
  Phase-0-era special-case prompt block; likely becomes a
  default-installed skill or a hook.
- Plugin sandboxing — out of scope for first cut; trust local
  install only.

---

## Milestone 2.7 — Subagent

### Why this is a milestone
Modern agent harnesses all provide a generic mechanism for spawning a
child agent loop with its own context window, system prompt, and tool
subset:

- Claude Code: `Task` tool with `agent_type` parameter, isolated context
- codex: sub-agent via separate session

Phase 0a removed leek's *4-persona* subagent because it was
over-specialized for an unproven use case. **The generic mechanism is
essential**, not optional, for:

- decomposing complex research (parallel branches)
- bounding context window per branch (each subagent has fresh /
  smaller context)
- tool subset isolation (a subagent gets only the tools it needs)
- skill-driven task delegation (a skill says "spawn a subagent with
  tool subset X")

### Scope (high-level, refine when M2.7 starts)

- A `task` (or equivalent name) tool that the main agent calls
- Subagent runs in its own loop with: its own system prompt (often
  skill-derived), its own tool registry subset, its own message
  history
- **Subagent's loop reuses _all_ M1 guards** (cost cap / wall-clock
  / idle / iteration / doom-loop / task_metrics)
- Result returned to parent as a single text block (not streamed,
  initially)
- Optional persona binding: `task(skill="equity-valuation",
  input="...")` loads the skill body as the subagent's system prompt
  + restricts tools to skill's `allowed_tools`
- Nesting: subagents can spawn subagents, with a default depth limit
  of 2 (main → child → grandchild stops there)

### Open questions
- **Tool name**: `task` (CC convention) vs `spawn_subagent`
  (descriptive) vs `delegate` (research-flavored). Default to CC's
  `task` unless we have reason to diverge.
- **Event streaming**: stream subagent events to parent in real time
  (richer UX) vs batch on completion (simpler). Likely batch first,
  stream later.
- **Depth limit default**: 2. Track via `task_metrics.depth`.
- **Subagent's vault scope**: shares parent's session_id? own
  session_id? Likely: own task_id under parent's session.

### Dependencies
- **Requires M1** — guards must work inside subagent loops too
  (otherwise a hung subagent corrupts the parent's UX).
- **Requires M2.5 skill** — persona binding via skill is a primary
  use case.
- **Blocks M3** — A-share task shapes ("full-market scan" + "deep
  stock review" parallel) want this.

---

## Milestone 3 — A-share MVP

### Goal (placeholder)
5–7 core A-share tools + 3 task shapes (e.g. quick scan, deep review,
comparison). The tools work, the prompts work, the task shapes are
repeatable.

### Already in place (Phase 0)
- `get_financials` — income / balance / cashflow / ratios
- `get_company_info` — profile + latest indicators
- `get_capital_flow` — moneyflow + northbound (with northbound-daily
  unavailability handled gracefully)
- `get_candlesticks` — OHLCV across markets
- `market_quote` — snapshot quotes

[Detailed scope expansion + task shape design deferred until M2.7
lands.]

---

## Milestone 4 — A-share complete

### Goal (placeholder)
Production-ready A-share research vertical: every common research
question shape covered, retention of conclusions across sessions,
observability dashboards.

[Detailed scope deferred.]

---

## Cross-cutting principles (apply to every milestone)

These are mandatory. If you find yourself violating one, stop and
raise it.

### 1. Tool naming neutrality (Phase 0e)
Tool names, advertised descriptions, parameter descriptions, output
footers, and error messages that the LLM (and through it the user)
reads must be **vendor-neutral**. Specific upstream identity
(Tushare, SEC EDGAR, Yahoo Finance, Binance, etc.) is operator /
developer concern only — it lives in code comments, struct fields,
env var names, and `tracing` log records, **never in anything the
model can learn from**.

Why: replaceability + separation of concerns + avoid dependency
lock-in via ergonomics.

### 2. Skill progressive disclosure (Phase 0d)
System prompt lists each skill's frontmatter `description` only (one
bullet per skill). Body is lazy-loaded via `use_skill(name)`. Don't
paraphrase or guess at a skill's content from the description alone
— the model loads it.

### 3. Trust the provider before constraining the loop
When in doubt, mirror codex's defaults. Hard caps误伤 too many real
cases (claude-code's removed 5-min timeout is the canonical lesson).
Capabilities are opt-in; observability is on by default.

### 4. Engineering decisions are the agent's; product decisions are the user's
The user delegates implementation choices to the agent. The agent
decides engineering (file layout, error handling, test shape, API
design). The user decides product fit (does this feature solve the
user's problem, what's the right UX, what should we cut).

### 5. Make engineering decisions visible
When the agent makes an engineering decision the user might want to
overrule, surface it in the response — don't bury it in a commit
message after the fact.

### 6. Prefer narrow before deep
M1 is wide and shallow (touch the loop infrastructure). M3 is narrow
and deep (one vertical, A-shares, done well). Don't expand
horizontally before the current narrow vertical proves itself.

---

## Decision log (chronological)

### 2026-05-09 — rebuild branch direction
- Tear out: critic / 4-persona subagent / decision_draft pipeline /
  budget_finalization.
- Keep: 4 panels (chat / canvas / corpus / plan).
- Approach: codex-style conventions where possible (auto-compaction,
  tool conventions, frontmatter skills).

### 2026-05-09 — soft-prompt + hard ceiling for time
- Wall-clock has both a soft prompt (block-level injection at
  10/5/2/1 min remaining) and a hard ceiling (30 min cancel).
- Soft is leek-original (no harness studied has it); hard is
  conservative (codex-level reluctance to cap, but still on by
  default at a wide ceiling).
- Cross-repo investigation: codex (no), claude-code (no, removed as
  bug), hermes-agent (no), openclaw (idle-only, not wall-clock).

### 2026-05-09 — opt-in vs default-on for guards
- Default-on: idle timeout, wall-clock, doom-loop, auto-compaction,
  observability (`task_metrics`).
- Opt-in: iteration cap, cost cap.
- Mirroring codex except where leek-original guards (doom-loop, time
  soft hints) make sense to ship on by default.
- Cross-repo investigation: codex (no iteration cap, no cost cap,
  90% auto-compact, no max_output_tokens), claude-code (no
  iteration cap, no per-call max_tokens default), openclaw
  (iteration cap [32, 160] scaled, idle timeout 60s, has retry
  cap), hermes-agent (no per-turn caps at all, only HTTP-level).

### 2026-05-09 — subagent added as M2.7
- Was missing from initial roadmap. Added between skill/hook/plugin
  (M2.5) and A-share MVP (M3) because subagent depends on skill
  machinery (persona binding) and unblocks A-share parallel task
  shapes.

### 2026-05-09 — `max_output_tokens` not sent (Phase 0f)
- codex-rs's `ResponsesApiRequest` struct has no field; CC and
  others trust provider per-model defaults. leek now matches this
  explicitly (field removed from `ChatRequest` struct, not just
  silently dropped at serialization).
