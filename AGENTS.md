# L.E.E.K (老韭菜)

**Logic-Enhanced Equity Kernel** — an investment research operating system for the long-suffering retail investor who wants to stop being market fodder.

## Project Identity

L.E.E.K is a **gateway-style agent system** (long-running daemon + multiple adapters: CLI / web / MCP HTTP / TUI / Claude Code skill / ACP) that turns a curated investing-wisdom corpus into actionable research, decisions, and post-mortems.

The CLI binary is named `leek`.

## Current Phase

**Requirements analysis + architecture design.** No implementation has started. All design work lives in [`design/`](design/).

Latest design state: [`design/handoff-2026-05-01.md`](design/handoff-2026-05-01.md). Read it first.

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
- Per-user runtime state (decisions, holdings, reviews, mandates) lives in a separate **vault**, never in the corpus and never in this repo. Default location TBD (`~/.leek/vault/` is the current working assumption — see handoff §5).

**The agent never writes directly to `corpus/wikis/` or `corpus/sources/`.** Promotion goes through `corpus/inbox/` with human review. This rule is day-1 multi-user-safety scaffolding even in solo mode.

## Cold-starting a Claude Code session here

When opening a new CC session in this project, do this in order:

1. Read this file (`AGENTS.md`).
2. Read [`design/handoff-2026-05-01.md`](design/handoff-2026-05-01.md) — full project context, prior consensus, and open decisions.
3. If `~/playground/finance-giant/corpus/` exists locally, glance at `finance-giant/corpus/AGENTS.md` to understand the corpus shape; otherwise the GitHub repo is `hchen13/the-corpus`.
4. Reference repos used as architecture sources may be at `~/research/repos/` (`dexter`, `warp`, `FinceptTerminal`, `hermes-agent`); clone if needed when the discussion calls for them.
5. Resume from the open decisions list at the end of the handoff.

## Working conventions

- All design / discussion artifacts go under `design/`.
- Cross-device handoff documents are named `handoff-YYYY-MM-DD.md`.
- Decision records (when we start logging them individually) go under `design/decisions/`.
- Scratch / temporary files go under `tmp/` (gitignored).

## Authoritative documents

This file and `design/handoff-2026-05-01.md` are authoritative for L.E.E.K. The corpus has its own authority (`finance-giant/corpus/AGENTS.md`) — when the two disagree about *the corpus*, that file wins; about *the agent*, this project wins.
