# `compaction-single-turn-burst` — M2.5 regression fixture

A real recorded leek session, captured against the live codex backend, that
M2.5 step 2 (rewriting the auto-compaction trigger) will use as ground truth
for a replay regression test. The recording is not synthetic — it's the raw
HTTP archive of an agent turn that actually ran.

This is one of **two** sibling fixtures the M2.5 step 1 dispatch produced.
See "Morphology" below — they cover two distinct compaction-related
pathologies and the step 2 replay test will exercise both.

## Morphology — single-turn burst

- **1 turn × 26 iter × 32 tool calls**, captured to completion.
- `metadata.json::turns[0].input_tokens = 1,916,163` — i.e. the per-turn
  `turn_metrics_recorded.input_tokens` (which is `Σ over iterations of
  usage.input_tokens`, `crates/gateway/src/agent/mod.rs:337`) ballooned
  past **7×** the model's 272K context window.
- `compaction_count = 0` for the same turn — codex's own
  `last_input_tokens` (the signal `compaction.rs` actually triggers on)
  never crossed the configured trigger fraction, despite the iter-sum
  blow-up.
- That gap **is** the pathology: a single dense turn can run up an enormous
  iter-sum without ever signalling "context is full" to the current
  trigger. step 2 needs to be able to distinguish iter-sum (a billing
  metric) from "current prompt size / context window" (the real trigger
  signal), and replay against this fixture is the test.

The paired fixture `crates/gateway/tests/fixtures/compaction-multi-turn-accumulation/`
covers the other shape: many shorter turns, chat-history accumulation
across turns rather than within a single turn. Together they bracket the
two real-world scenarios where the trigger needs to behave correctly.

## What this fixture is for

`compaction.rs` currently triggers on `last_input_tokens`, the value codex
reports in its most recent `usage.input_tokens`. That signal turns out to be
polluted by codex's in-call `web_search` browsing — the same value sums
leek-side prompt context (which compaction can shrink) with codex-side page
fetches (which it cannot). Until the trigger signal is fixed, M1.8's
compaction code has never actually fired on a real long session.

M2.5 step 1 (this dispatch) records what a real long session looks like.
M2.5 step 2 will:

1. Add a unit / integration test that replays this fixture against the new
   compaction implementation and asserts the trigger fires at the expected
   iteration boundary.
2. Verify the rewritten signal separates leek-context tokens from
   codex-browsing tokens.

## When this fixture needs to be re-recorded

Re-record whenever any of these change in a way that would invalidate the
captured request bodies or token counts:

- **Corpus is materially re-shaped** — sources added/removed/restructured.
  The agent's tool calls reference corpus paths; a different corpus produces
  different tool I/O.
- **Codex model upgrade** — moving off `gpt-5.5` to a successor changes both
  the system-prompt format and the per-iteration token accounting.
- **Tokenizer / pricing table** changes for the configured model — the per-
  iteration `usage.input_tokens` and `cost_usd` values shift.
- **System prompt rewrite** — `prompt::build_system_prompt` is the
  per-iteration `instructions` field; a non-trivial rewrite invalidates the
  request-body byte equivalence the replay test relies on.

A pure tools surface change (adding a new tool, renaming one, tweaking
schemas) does NOT in itself require a re-record — the fixture is checked by
content, not by tool inventory. But if the change is large enough that the
recorded turn no longer represents typical M2-era behavior, re-record.

## How this fixture was recorded (v1 of the recorder)

The single-turn-burst shape is what dropped out when the recorder used a
since-removed `INPUT_TOKEN_STOP=200000` stop condition — the v1 dispatch
had mistaken the iter-sum `input_tokens` metric for a context-window
utilisation proxy. The first turn tripped the cap at iter 26 and the
recorder stopped. v2 of the recorder (`scripts/record-compaction-fixture.sh`)
no longer carries `INPUT_TOKEN_STOP`; to reproduce this burst shape, run
v1's variant of the script (recoverable from git history at the commit that
landed this fixture) or restore the threshold knob.

## How to re-record (current v2 recorder)

```bash
# 1. backend running on :8964 with valid codex OAuth + populated corpus
cargo run --bin leek -- serve

# 2. drive the recorder (one-shot — DO NOT restart leek mid-recording)
caffeinate -i ./scripts/record-compaction-fixture.sh
```

Threshold knobs (env vars on the v2 script):

| var                  | default     | meaning                                             |
|----------------------|-------------|-----------------------------------------------------|
| `MAX_TURNS`          | `30`        | hard cap on prompt-list iteration                   |
| `COST_CAP_USD`       | `200`       | stop when cumulative `cost_usd` ≥ this              |
| `TURN_TIMEOUT_SEC`   | `600`       | per-turn poll timeout                               |
| `POLL_SEC`           | `3`         | event-log poll interval                             |
| `OUTPUT_DIR`         | multi-turn  | output directory                                    |

The recorder reads prompts from an embedded array (`PROMPTS=(…)` in the
script). v2's sequence is intentionally tuned to **constrain per-turn iter
count** so the multi-turn-accumulation shape emerges; to re-produce the
single-turn-burst shape, override `PROMPTS` with the v1 array (which asked
for exhaustive corpus extraction in a single prompt).

## What's in this fixture (current recording)

- **Recorded**: 2026-05-21 (see `metadata.json::recorded_at`)
- **leek git rev**: see `metadata.json::leek_git_rev`
- **codex model**: `gpt-5.5` (272K context window)
- **Session**: `metadata.json::session_id`
- **Turns captured**: 1 (see `metadata.json::turns`)
- **Stop reason**: `input_tokens_threshold` (v1 recorder behavior)

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
