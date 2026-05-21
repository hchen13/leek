# `compaction-multi-turn-accumulation` — M2.5 regression fixture

A real recorded leek session, captured against the live codex backend,
that M2.5 step 2 (rewriting the auto-compaction trigger) will use as
ground truth for a replay regression test. The recording is not
synthetic — it's the raw HTTP archive of 30 sequential turns that
actually ran in one session.

This is one of **two** sibling fixtures the M2.5 step 1 dispatch
produced. See "Morphology" below — they cover two distinct
compaction-related pathologies and the step 2 replay test will exercise
both.

## Morphology — multi-turn accumulation

- **30 turns × ~3.4 iter/turn (avg)**, captured end-to-end. The recorder
  drove a Buffett value-investing prompt sequence (`scripts/record-compaction-fixture.sh`'s
  `PROMPTS` array, v2's 调淡 form) over one continuous session.
- Iter distribution per turn:
  - `iter=1`: 6 turns · `iter=2`: 5 turns · `iter=3`: 13 turns (mode) ·
  - `iter=4-9`: 5 turns · `iter=17`: 1 turn (single burst outlier on
    turn 3, the historical-research prompt that probed Buffett 1965-1975)
- **Total iterations: 103**, total tool calls: 96 (`tool_lifecycle`/2),
  search calls: 49 (`search_lifecycle`/2). chat_messages history grows
  monotonically across turns — by turn 30, the prompt that goes to codex
  includes 29 prior assistant replies + 30 user messages + system +
  re-played function_call dialogues.
- `compaction_count = 0` for every turn. Despite 29 turns of chat
  history accumulation, `last_input_tokens` (the signal `compaction.rs`
  triggers on) never crossed the configured fraction. That is the
  property M2.5 step 2 needs to flip — the rewritten estimator should
  see "leek-side context filling up" and fire compaction somewhere in
  the back half of this fixture, while the current trigger does not.

The paired fixture
`crates/gateway/tests/fixtures/compaction-single-turn-burst/` covers
the opposite shape: a single dense turn (26 iter / 32 tool calls) where
the per-turn `input_tokens` metric balloons within one turn rather
than accumulating across turns. Together they bracket the two
real-world scenarios where the trigger needs to behave correctly.

## What this fixture is for

`compaction.rs` currently triggers on `last_input_tokens`, the value
codex reports in its most recent `usage.input_tokens`. That signal is
polluted by codex's in-call `web_search` browsing — the same value
sums leek-side prompt context (which compaction can shrink) with
codex-side page fetches (which it cannot). Until the trigger signal is
fixed, M1.8's compaction code has never actually fired on a real long
session.

M2.5 step 1 (this dispatch) records what real long sessions look like.
M2.5 step 2 will:

1. Add a unit / integration test that replays this fixture against the
   new compaction implementation and asserts the trigger fires at the
   expected iteration boundary.
2. Verify the rewritten signal separates leek-context tokens from
   codex-browsing tokens.

The replay test will likely use `transcripts/turn-N-iter-M-request.json`
as the input the new estimator scores, and assert it crosses threshold
somewhere between turn ~22 and turn ~30 (where chat history is
substantial enough to fill the model window).

## When this fixture needs to be re-recorded

Re-record whenever any of these change in a way that would invalidate
the captured request bodies or token counts:

- **Corpus is materially re-shaped** — sources added/removed/restructured.
  The agent's tool calls reference corpus paths; a different corpus
  produces different tool I/O.
- **Codex model upgrade** — moving off `gpt-5.5` to a successor changes
  both the system-prompt format and per-iteration token accounting.
- **Tokenizer / pricing table** changes for the configured model — the
  per-iteration `usage.input_tokens` and `cost_usd` values shift.
- **System prompt rewrite** — `prompt::build_system_prompt` is the
  per-iteration `instructions` field; a non-trivial rewrite invalidates
  the request-body byte equivalence the replay test relies on.

A pure tools surface change (adding a new tool, renaming one, tweaking
schemas) does NOT in itself require a re-record — the fixture is
checked by content, not by tool inventory.

## How to re-record

```bash
# 1. backend running on :8964 with valid codex OAuth + populated corpus
cargo run --bin leek -- serve

# 2. drive the recorder (one-shot — DO NOT restart leek mid-recording).
#    caffeinate -i prevents macOS from sleeping during the long run.
caffeinate -i ./scripts/record-compaction-fixture.sh
```

Threshold knobs (env vars on the script):

| var                  | default                              | meaning                                              |
|----------------------|--------------------------------------|------------------------------------------------------|
| `MAX_TURNS`          | `30`                                 | hard cap on prompt-list iteration                    |
| `COST_CAP_USD`       | `200`                                | stop when cumulative `cost_usd` ≥ this               |
| `TURN_TIMEOUT_SEC`   | `600`                                | per-turn poll timeout                                |
| `POLL_SEC`           | `3`                                  | event-log poll interval                              |
| `OUTPUT_DIR`         | this dir                             | output directory                                     |
| `RESUME_SESSION_ID`  | unset                                | resume into an existing leek session                 |
| `RESUME_FROM_PROMPT` | `1`                                  | when resuming, the 1-indexed prompt to post next     |

Resume mode (used to recover from a mid-run kill) skips the first
`RESUME_FROM_PROMPT - 1` prompts and replays prior turns' state by
reading the persisted event log; the previously-completed transcripts
are then naturally re-included by the export step. This fixture was
recorded across one initial run (prompts 1-7) + one resume run
(prompts 8-30) under the same `RESUME_SESSION_ID`.

The recorder reads prompts from an embedded array (`PROMPTS=(…)` in the
script). v2's sequence intentionally constrains per-turn iter count so
the multi-turn-accumulation shape emerges; to re-produce the
single-turn-burst shape, override `PROMPTS` with the v1 array (asking
for exhaustive corpus extraction in one prompt).

## What's in this fixture

- **Recorded**: 2026-05-21 (see `metadata.json::recorded_at`)
- **leek git rev**: see `metadata.json::leek_git_rev` (`4ab3514` —
  M2-polish HEAD)
- **codex model**: `gpt-5.5` (272K context window)
- **Session**: `metadata.json::session_id`
- **Turns captured**: 30 (see `metadata.json::turns`)
- **Stop reason**: `prompts_exhausted`
- **Total cost**: see `metadata.json::total_cost_usd`
- **Total wall clock**: see `metadata.json::total_wall_clock_ms`
- **Compaction count (sum across turns)**: 0

```
metadata.json     — per-turn metrics + run-level summary
session.json      — session row + extracted system prompt + model
events.json       — every persisted event the session emitted (no
                    `assistant_delta` — those are SSE-ephemeral by design)
transcripts/      — raw codex request body + SSE response per iteration:
  turn-<N>-iter-<M>-request.json     application/json (verbatim Responses-API body)
  turn-<N>-iter-<M>-response.txt     text/event-stream (verbatim SSE bytes)
```

`turn-<N>` is 1-indexed and matches `metadata.json::turns[N-1].turn_id`.
`iter-<M>` is the in-turn iteration index from the agent loop (1-based).
