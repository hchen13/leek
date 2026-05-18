# L.E.E.K (老韭菜)

**Logic-Enhanced Equity Kernel** — an investment research operating system for the long-suffering retail investor who wants to stop being market fodder.

## Project Identity

L.E.E.K is a **gateway-style agent system** (long-running daemon + multiple adapters: CLI / web / MCP HTTP / TUI / Claude Code skill / ACP) that turns a curated investing-wisdom corpus into actionable research, decisions, and post-mortems.

The CLI binary is named `leek`.

## Project Shape

L.E.E.K is not just an investment prompt collection. It is a **harness** for
corpus-grounded investment agents: the durable work is in loop control, tool
surfaces, state, evidence, provenance, plan semantics, budgets, recovery, and
human confirmation boundaries.

Implementation state should be read from the code. Product and UX boundaries
are locked in [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md). End-state
architecture is in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), and
milestone order / completion state is in [`docs/MILESTONES.md`](docs/MILESTONES.md).
The old [`design/`](design/) tree is historical reference only unless a current
docs file explicitly re-adopts a detail from it.

## Relationship to `finance-giant/` and the corpus

The two projects are **deliberately separate repos sitting side-by-side** at `~/playground/`:

```
~/playground/
├── finance-giant/           # corpus + raw-material collection (this is NOT leek)
│   └── corpus/              # the LLM wiki — read-only data layer for leek
└── leek/                    # this project — the agent system
    ├── AGENTS.md
    └── design/
```

- **`finance-giant/corpus/`** is universal, slow-moving, curated investing wisdom (Buffett / Munger / Dalio at minimum). It is L.E.E.K's *read-only* knowledge layer.
- **`leek/`** is the agent system: gateway daemon, adapters, scheduler, decision/position tracker, promotion pipeline.
- Per-user runtime state (decisions, holdings, reviews, mandates) lives in a separate **vault**, never in the corpus and never in this repo.

**The agent never writes directly to `corpus/wikis/` or `corpus/sources/`.** Promotion goes through `corpus/inbox/` with human review. This is multi-user-safety scaffolding even in solo mode.

## Cold-starting an agent session here

When opening a new agent session in this project, do this in order:

1. Read this file (`AGENTS.md`).
2. Read [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md), then the current code paths relevant to the task.
3. Use [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for architectural principles and [`docs/MILESTONES.md`](docs/MILESTONES.md) for phase boundaries.
4. Treat `design/` as archive material. Do not follow old task / deliverable / mandate / portfolio / `LlmProvider` / Reasoning DAG specs unless the current docs explicitly say to reintroduce that idea.
5. If `~/playground/finance-giant/corpus/` exists locally, glance at `finance-giant/corpus/AGENTS.md` to understand the corpus shape; otherwise the GitHub repo is `hchen13/the-corpus`.
6. Reference repos used as architecture sources may be at `~/research/repos/` (`dexter`, `warp`, `FinceptTerminal`, `hermes-agent`); clone if needed when the discussion calls for them.
7. For agent-loop, plan, tool, subagent, skill, budget, context, or reliability work, read [`.agents/skills/harness-engineering/SKILL.md`](.agents/skills/harness-engineering/SKILL.md) before proposing changes.
8. Resume from the current task request and current code state.

## Working conventions

- Current product, UX, architecture, and milestone artifacts go under `docs/`.
- The existing `design/` tree is historical archive material. Do not add new authoritative specs there.
- Scratch / temporary files go under `tmp/` (gitignored).
- **Browser/Playwright 截图必须保存到 `tmp/`，严禁落到项目根目录。** 测试结束后 `tmp/` 内容可直接清空，不需逐一确认。

## Authoritative documents

Authority order for L.E.E.K itself:

1. Current code.
2. [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md) for product / UX / acceptance requirements.
3. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for architecture.
4. [`docs/MILESTONES.md`](docs/MILESTONES.md) for phase order and completion state.
5. `design/` only as non-authoritative historical reference.

The corpus has its own authority (`finance-giant/corpus/AGENTS.md`) — when the two disagree about *the corpus*, that file wins; about *the agent*, this project wins.
