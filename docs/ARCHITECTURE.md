# L.E.E.K Architecture

> **End-state spec for the rebuild-clean branch.**
>
> Companion to `docs/MILESTONES.md`. MILESTONES tells you *what to ship
> when*; this file tells you *what the system looks like once it's
> all shipped*. Keep both in sync — when an architectural decision
> changes here, find the milestone that owns it and update there too.
>
> Last revised: 2026-05-11, at the rebuild-clean reset.

---

## 1. Mission

L.E.E.K is a **domain-specialized AI agent for investment research**.

Structurally it is a near-clone of codex / Claude Code:

- one main agent loop,
- a registry of tools,
- a small set of built-in skills,
- the modern harness primitives: skill / hook / plugin,
- and a subagent-spawn mechanism for delegating to specialized child
  agents.

The differentiation is **not** structural. It is content:

- a curated investment **corpus** (markdown knowledge base, injected
  into the main agent's system prompt and queryable via tools);
- a small set of **domain tools** (quote / fundamentals / news /
  capital-flow lookups);
- **user mandate** — the user's portfolio, risk tolerance, style,
  horizon — collected once and persistent across sessions.

Everything else is the same agent harness pattern the broader
industry has converged on. We do not invent agent-loop primitives.
We adopt them.

---

## 2. Non-goals (explicit)

These were present in earlier leek revisions and are deliberately
absent from rebuild-clean:

- **No routing layer.** Every user message enters the main loop. No
  upstream LLM classifies messages into `new_task` / `chat_reply` /
  `ambiguous`. The model itself decides whether to call tools.
- **No deliverable taxonomy.** No `research_brief` / `comparison` /
  `morning_brief` / `free_form` categorization. The model produces
  whatever shape of output the user asked for.
- **No `task` entity in the vault.** Conversations are sessions of
  messages. There is no LLM-classified "task" record sitting between
  the session and its messages.
- **No `plan_guard` enforcement.** The model uses `update_plan` if it
  wants to, and ships its answer when it's ready. We don't second-guess
  by intercepting "the plan isn't done yet".
- **No 4-persona subagent.** There is exactly one *generic* subagent
  mechanism. Specialization comes from skill bodies, not from
  hardcoded personas.
- **No LLM provider abstraction.** Today there is only one path to
  models: codex pro via OAuth. The codebase wires directly to that.
  Re-abstract when there's a second concrete provider with a real
  contract — not before.
- **No charters / decisions / portfolio / holdings as first-class DB
  entities** (in the M0-M1 cut). User mandate is a single text blob
  injected into the system prompt. Promotion to structured fields
  happens only if/when a milestone needs it.

These were not arbitrary. Each was tried, observed to add latency,
bug surface, or cognitive load disproportionate to its value, and
removed.

---

## 3. System shape

```
+-----------------------+        +-----------------------+
|  Frontend (Solid)     | <----> | Gateway (Rust)        |
|  - chat panel         |  SSE   | - HTTP + SSE          |
|  - corpus viewer      |  HTTP  | - auth (OAuth)        |
|  - plan / canvas      |        | - vault (SQLite)      |
|                       |        | - main agent loop     |
+-----------------------+        | - subagent spawn      |
                                 | - tool registry       |
                                 | - corpus loader       |
                                 | - skill / hook engine |
                                 +-----------------------+
                                            |
                                            v
                                 +-----------------------+
                                 | codex pro (OAuth)     |
                                 | Responses API         |
                                 +-----------------------+
```

- **Single Rust crate** (`crates/gateway`) hosts everything backend-side.
  No multi-crate split until we have a real reason (a CLI peer, an
  extracted SDK, etc.).
- **Single LLM access path**: codex OAuth → Responses API. No
  `LlmProvider` trait, no `OpenAIClient` / `AnthropicClient` parallel
  hierarchy. Just one concrete client.
- **Single vault**: per-user SQLite file. Tables (M0): `users`,
  `sessions`, `messages`, `user_settings`. New tables get added only
  by an explicit milestone that needs them, with the migration
  reviewed at commit time.
- **SSE for streaming**: model output, tool events, plan updates all
  ride a single SSE channel per session.

---

## 4. Agent topology

### 4.1 Main agent

Runs in the user's session. One per active session.

System prompt assembly (in order, each section optional):

1. **Identity** — short, leek-specific framing. Stable text.
2. **User mandate** — the user's investment profile. Injected
   verbatim if present.
3. **Corpus orientation** — short curated snippets from the corpus
   that always apply (principles, definitions). Long-form corpus is
   loaded on demand via tools, not in the prompt.
4. **Skill index** — one line per discovered skill: `name —
   frontmatter description`. Bodies are lazy-loaded via `use_skill`.
5. **Available tools** — names + first-line descriptions, sourced
   from the tool registry. Orienting context only — the model picks
   tools from the API `tools` array, not from this list.

What's deliberately not in the system prompt:

- No prose telling the model *when* to use which tool. Tool
  descriptions in the registry are the source of truth.
- No deliverable framing. The model produces what was asked for.
- No plan-required nag. `update_plan` is offered as a tool; the
  model uses it if it helps.

### 4.2 Subagents

Spawned by the main agent (or by another subagent, up to depth=2)
via the `task` tool — CC convention.

Each subagent gets:

- its own system prompt (often a skill body),
- its own tool subset (often the skill's `allowed_tools`),
- its own context window (fresh, not inheriting parent's),
- its own loop instance running the same code as the main loop —
  same safety nets, same metrics.

Communication is **one-shot**: the parent passes a prompt, the
subagent returns one text block. No streaming back to parent in v0
(can add later if useful).

Initial subagent shapes — populated as we build:

- **`corpus-expert`** — system prompt is "you know the corpus
  deeply, the user asks a corpus-grounded question, return a synthesis
  with direct quotes". Tool subset: `corpus_search`, `corpus_read`.
  Analogue: CC's `claude-code-guide`.
- **`market-data-fetcher`** — parallelizable lookups across a list
  of tickers. Tool subset: market / fundamentals tools.
- **`planner`** — multi-step research decomposition. Tool subset:
  minimal (no data fetching) — returns a plan, doesn't execute.

New subagents arrive as skills under `harness/skills/<name>/SKILL.md`
once the skill mechanism lands (M2.5). Hardcoded subagents (the three
above) only as a bootstrap, replaced by skill-discovered ones at the
first opportunity.

### 4.3 Why subagents matter for investment

- **Context window discipline** — A "deep review of NVDA" can pull
  20 documents from the corpus, 6 quote snapshots, 4 quarters of
  financials. Doing it all in the main loop blows the context. A
  subagent does the heavy fetching, returns the digest, parent keeps
  a clean context.
- **Parallelism** — A multi-ticker scan ("compare BABA / 9988.HK /
  PDD / JD") naturally factors into N parallel subagent calls.
- **Specialization without persona zoo** — `corpus-expert` is a
  prompt + tool subset, not a hardcoded code path. Adding
  `crypto-research-expert` or `event-driven-trader` is editing
  markdown, not Rust.

---

## 5. Harness primitives

These are what every modern agent harness has. We adopt them
wholesale; the M1 work that landed on the prior `rebuild` branch
informs the implementation but is re-done on rebuild-clean's cleaner
foundation.

| Primitive            | Default                | Scope                  | Notes                                                                                |
|---------------------|------------------------|------------------------|--------------------------------------------------------------------------------------|
| Idle timeout         | 90 s, on               | per-stream             | Mirrors CC's `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`. Primary responsiveness guard.   |
| Wall-clock ceiling   | 30 min, on             | per-turn               | Hard cancel. Soft prompts at 10/5/2/1 min remaining (leek-original).               |
| Iteration cap        | None, opt-in           | per-turn               | Codex / CC don't enforce. Available for power users; off by default.               |
| Cost cap             | None, opt-in           | per-turn               | USD ceiling using per-model price table.                                            |
| Doom-loop detector   | N=3, on                | per-turn               | Identical `(tool_name, args)` ≥ N consecutive calls → abort.                       |
| Auto-compaction      | 90%, on                | per-session            | Mirrors codex.                                                                       |
| Per-turn metrics     | on                     | per-turn               | Single row per turn: stop_reason, tokens, cost, first_triggered_guard, iteration.  |

Subagent loops reuse **all** of these. A subagent that hangs must
not corrupt the parent's UX — guards are uniform.

Naming note: a *turn* is one user prompt → one final assistant
response. An *iteration* is one LLM call within a turn. The metrics
table is per-turn, not per-iteration.

---

## 6. Storage (vault)

Per-user SQLite. M0 schema:

```sql
users(id, oauth_subject, created_at)
sessions(id, user_id, title, created_at, last_active_at)
messages(seq, session_id, user_id, role, content, created_at, ...)
user_settings(user_id, ...)
turn_metrics(turn_id, session_id, ...)   -- M1
```

Tables intentionally absent in M0:
`tasks`, `deliverables`, `charters`, `decisions`, `plans`,
`holdings`, `provider_configs`, `compactions`, `tool_runs`,
`subagents` (subagent spawn doesn't need its own table —
turn_metrics carries `parent_turn_id`).

Each table that joins comes with **why now** in its migration
header. If a table doesn't have a written justification at addition
time, that's a code review reject.

---

## 7. User mandate

The investment-domain analogue of "what the user wants out of this
agent". Examples of what it captures:

- **Holdings** (tickers, sizes, costs)
- **Risk tolerance** (stated)
- **Style preferences** (growth / value / GARP / event-driven / etc.)
- **Time horizon** (day-trader / position / long-term)
- **Currencies and markets** the user actually trades
- **Hard constraints** ("no leveraged products", "no crypto", etc.)

How we plan to handle it (subject to refinement — see Open Questions):

- **Persistence**: `user_settings.mandate_text` — a single markdown
  blob, user-editable, versioned in vault.
- **Injection**: included verbatim in the main agent's system prompt
  (Section 4.1, position 2).
- **Collection**: onboarding skill (`harness/skills/mandate/`) — runs
  on first session, asks 4–6 questions, writes the result. Editable
  later via "edit mandate" in settings.
- **Subagent visibility**: subagents do *not* automatically inherit
  mandate. If a subagent needs it, the parent passes the relevant
  slice in its task prompt.

**This is the part of the design we're least sure about** (see §11).

---

## 8. Corpus

Two surfaces:

1. **Default injection** in the main agent's system prompt.
   - A small, curated set of corpus snippets — principles,
     definitions, recurring framings.
   - Capped (target: < 800 tokens). Long-form goes through tools.
2. **Tools** for on-demand retrieval.
   - `corpus_search(query) → [hit{id, title, snippet}]`
   - `corpus_read(id) → full body`
   - `corpus-expert` subagent for question-shape queries that need
     synthesis across multiple corpus docs.

Storage: markdown files under a corpus root. Versioned in git for
now; later the structure can move to a content management surface
(M2 open question).

Retrieval: lexical (BM25) in v0. Embeddings later if recall is bad.

---

## 9. Skill / Hook / Plugin

CC conventions, adapted minimally.

**Skill** (`harness/skills/<name>/SKILL.md`):

- Frontmatter: `name`, `description`, optional `allowed_tools`, `model`
- Body: free-form markdown, loaded on demand via `use_skill(name)`
- Discovery from `harness/skills/` (bundled) + user dir
  (`~/.leek/skills/`) + project dir (`<project>/.leek/skills/`)
- Hot reload via `notify` watcher

**Hook** events (match CC's surface):

`PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`,
`SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreCompact`,
`Notification`

Hook execution: shell command, captured stdout / exit code,
configurable timeout.

**Plugin**: a bundle of skills + hooks + a manifest. Local install
only in v0. Remote / marketplace later.

---

## 10. What stays vs goes from the old `rebuild` branch

This is a literal mapping for the cleanup commit(s). The bar for
"stays": minimal MVP needs it directly. Anything else gets cut.

### Stays (with possible refactor)

- `crates/gateway/src/main.rs` — entry point
- `crates/gateway/src/auth/` — OAuth login
- `crates/gateway/src/llm/codex_oauth.rs` — OAuth flow
- `crates/gateway/src/llm/openai_responses.rs` — Responses API client
- `crates/gateway/src/llm/pricing.rs` — per-model price table (for
  cost cap)
- `crates/gateway/src/events/` — SSE infrastructure
- `crates/gateway/src/api/sessions.rs` — session CRUD (will simplify)
- `crates/gateway/src/api/messages.rs` — message POST + stream
- `crates/gateway/src/api/static_files.rs` — frontend serving
- `crates/gateway/src/api/health.rs` — health check
- `crates/gateway/src/vault/mod.rs`, `sessions.rs`, `messages.rs`,
  `user_settings.rs`
- `crates/gateway/src/corpus/` — corpus loader (will simplify)
- `crates/gateway/migrations/0001_initial.sql` — user / session /
  message schema
- `crates/gateway/migrations/0007_user_settings.sql` — user settings
- `frontend/web/` — entire frontend tree, API contract adjusted
- `harness/skills/` — bundled skills (will rewrite bodies as we
  rebuild the agent that consumes them)
- `harness/identity.md`, `harness/discipline.md`,
  `harness/corpus_orientation.md` — system prompt fragments
- `corpus/` — corpus content (currently empty placeholder)

### Goes (DELETE)

- `crates/gateway/src/agent/mod.rs` (2276 lines) — to be rewritten
  small
- `crates/gateway/src/agent/harness.rs` — system prompt builder
  rewrite
- `crates/gateway/src/agent/routing.rs` — routing layer, delete
  entirely
- `crates/gateway/src/agent/compact.rs` — auto-compact, fold into
  new loop in M1
- `crates/gateway/src/agent/tools/` — keep the *tool definitions*
  but redo the registry plumbing
- `crates/gateway/src/api/charter.rs`
- `crates/gateway/src/api/corpus.rs` (keep tool, drop dedicated API
  if unused)
- `crates/gateway/src/api/deliverables.rs`
- `crates/gateway/src/api/portfolio.rs`
- `crates/gateway/src/api/tools.rs` (legacy)
- `crates/gateway/src/api/stream.rs` (if redundant with events/)
- `crates/gateway/src/vault/charters.rs`
- `crates/gateway/src/vault/compactions.rs`
- `crates/gateway/src/vault/decisions.rs`
- `crates/gateway/src/vault/holdings.rs`
- `crates/gateway/src/vault/plans.rs`
- `crates/gateway/src/vault/provider_configs.rs`
- `crates/gateway/src/vault/subagents.rs`
- `crates/gateway/src/vault/task_metrics.rs` — renamed to
  `turn_metrics.rs` in M1
- `crates/gateway/src/vault/tasks.rs`
- `crates/gateway/src/vault/tool_runs.rs`
- `crates/gateway/migrations/0002_compaction.sql`
- `crates/gateway/migrations/0003_decisions.sql`
- `crates/gateway/migrations/0004_charters.sql`
- `crates/gateway/migrations/0005_in_place_compaction.sql`
- `crates/gateway/migrations/0006_agent_plan_items.sql`
- `crates/gateway/migrations/0008_decision_structure.sql`
- `crates/gateway/migrations/0009_plan_resolution.sql`
- `crates/gateway/migrations/0010_task_metrics.sql` — renamed +
  reshaped in M1

### Frontend reshape (M0)

- Remove cards / panels tied to deleted backend entities (charter
  panel, decision draft card, portfolio panel, deliverable
  artifacts, task-status indicators).
- Keep: chat composer, message list, SSE streaming wiring, corpus
  viewer, plan view.
- API contract adjusts to the new gateway surface. Specifics in M0.

---

## 11. Open questions

These don't block the rebuild start. They're tracked so they
surface when their owning milestone arrives.

### User mandate (M2)
- **Onboarding UX**: skill-driven Q&A vs settings form vs both?
- **Mutability**: in-chat (`/edit-mandate`) vs settings page vs both?
- **Mandate length cap**: a mandate that grows to 2K tokens eats
  every system prompt. Hard cap, or summarization pass on save?
- **Subagent mandate visibility**: when does a subagent get mandate?
  Always? Opt-in by skill? Decided per task by main agent?

### Corpus (M2)
- Embedding vs lexical retrieval — lexical for v0, but at what
  recall floor does embedding become necessary?
- Versioned corpus authoring UX — GitOps (PR each change)? In-app
  editor? Both?

### Subagent (M2.7)
- Tool name: `task` (CC) vs `delegate` (research-flavored) vs
  `spawn_subagent` (descriptive). Default `task`.
- Event streaming: batch result to parent v0, stream later — but
  what's the breakpoint where streaming becomes worth the
  complexity?
- Subagent vault scope: own task_id within parent's session, or
  its own session_id? Default: own turn within parent session, no
  separate session entity.

### Skill / Hook / Plugin (M2.5)
- Plugin sandboxing — punt to "trust local installs only" in v0,
  but at first remote-plugin desire we have to revisit.
- Skill model override semantics when paired with codex OAuth (CC
  routes per-skill model; codex pro is one model).

### Codex OAuth specifics
- Token refresh edge cases — what happens when refresh fails
  mid-stream? Where's the retry boundary?
- Rate limit observability — when codex pro throttles, how do we
  surface it without leaking provider identity (per principle 1)?

---

## 12. Cross-cutting principles

Carried forward from `rebuild` MILESTONES.md §"Cross-cutting principles".
These are mandatory. They survived the rebuild-clean reset because
they're about *how to build*, not *what to build*.

1. **Tool naming neutrality.** Tool names, descriptions, parameter
   docs, error messages: vendor-neutral. Provider identity
   (Tushare, SEC EDGAR, etc.) lives in code comments, struct fields,
   env var names, and `tracing` logs — never in anything the model
   reads.

2. **Skill progressive disclosure.** System prompt lists each skill's
   frontmatter `description` only. Body is lazy-loaded via
   `use_skill(name)`. Don't paraphrase or guess at a skill's content
   from the description alone — the model loads it.

3. **Trust the provider before constraining the loop.** Mirror codex
   defaults when in doubt. Hard caps mis-fire on too many real cases.
   Capabilities are opt-in; observability is on by default.

4. **Engineering decisions are the agent's; product decisions are
   the user's.** Implementation choices delegated. Product fit, UX
   shape, feature priority — user's call.

5. **Make engineering decisions visible.** When making an
   engineering decision the user might want to overrule, surface it
   in the response — don't bury it in a commit message.

6. **Prefer narrow before deep.** M1 is wide and shallow (loop
   infrastructure). M3 is narrow and deep (A-shares first). Don't
   expand horizontally before the current narrow vertical proves
   itself.

7. **(new) Don't bring deterministic-systems thinking into an
   agent.** This is what cost us the prior rebuild. A web app
   workflow has state machines and validation gates because users
   click buttons in known orders. An agent has a model deciding
   what to do next. Routing layers, deliverable taxonomies, plan
   guards that intercept the model — all of these tried to *force*
   determinism onto the agent and produced bug surfaces. The agent
   harness is a loop and a tool list. Anything else gets a high
   bar: "would codex or CC have this? if not, why does leek need
   it?"
