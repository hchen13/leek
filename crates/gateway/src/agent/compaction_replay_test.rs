//! Replay regression tests for auto-compaction (M2.5 step 2).
//!
//! Each test loads one of the step-1 fixtures
//! (`crates/gateway/tests/fixtures/compaction-*`), replays every recorded
//! iteration through the post-M2.5 trigger signal, and asserts the
//! compaction fires at the expected point and folds the context the way
//! production would.
//!
//! Why these tests live as a `cfg(test)` module rather than under
//! `tests/`: this crate ships only a binary, so an integration test
//! could not see `compaction::{plan_split_for, apply_compaction,
//! estimate_context_tokens}` without first adding a `lib.rs` to publish
//! them. The dispatch (M2.5 step 2 `Replay regression test`) explicitly
//! allows in-module placement, and keeping the replay alongside the
//! production logic avoids that extra surface.
//!
//! The summarizer is **never** invoked here. `compaction::compact` is
//! split into `plan_split_for` + `apply_compaction` so a test can drive
//! the fold with a mock summary string and exercise the same in-place
//! mutation production runs — without burning real codex tokens.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::compaction::{
    apply_compaction, estimate_context_tokens, plan_split_for, TurnContext,
};
use super::tools;
use crate::llm::{ChatMessage, Role};

fn fixture_root(dirname: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(dirname);
    p
}

/// One iteration's recorded state — pulled from a single
/// `turn-N-iter-M-request.json`. Splits the recorded `input` array into
/// the chat messages and the re-injected tool dialog the way the agent
/// loop carries them, so the replay can feed them straight into the
/// trigger.
struct IterState {
    turn: u32,
    iter: u32,
    instructions: String,
    messages: Vec<ChatMessage>,
    tool_dialog: Vec<Value>,
}

fn parse_request(path: &Path) -> IterState {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap();
    let (turn, iter) = parse_turn_iter(stem);
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let req: Value = serde_json::from_str(&raw).expect("request.json is JSON");
    let instructions = req
        .get("instructions")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mut messages = Vec::new();
    let mut tool_dialog = Vec::new();
    if let Some(arr) = req.get("input").and_then(|v| v.as_array()) {
        for item in arr {
            match item.get("type").and_then(|t| t.as_str()) {
                None => {
                    // ChatMessage — `{role, content}` with no `type` tag.
                    let role = item
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("user");
                    let content = item
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let role = match role {
                        "user" => Role::User,
                        "assistant" => Role::Assistant,
                        _ => Role::Developer,
                    };
                    messages.push(ChatMessage { role, content });
                }
                Some(_) => {
                    // function_call / function_call_output / other items
                    // — preserved verbatim as the agent loop does.
                    tool_dialog.push(item.clone());
                }
            }
        }
    }
    IterState {
        turn,
        iter,
        instructions,
        messages,
        tool_dialog,
    }
}

fn parse_turn_iter(stem: &str) -> (u32, u32) {
    // Stems look like `turn-12-iter-3-request`.
    let mut parts = stem.split('-');
    let _ = parts.next(); // "turn"
    let turn: u32 = parts.next().unwrap().parse().unwrap();
    let _ = parts.next(); // "iter"
    let iter: u32 = parts.next().unwrap().parse().unwrap();
    (turn, iter)
}

fn load_fixture_iters(dirname: &str) -> Vec<IterState> {
    let dir = fixture_root(dirname).join("transcripts");
    let mut requests: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.ends_with("-request.json"))
                .unwrap_or(false)
        })
        .collect();
    requests.sort_by_key(|p| {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap();
        parse_turn_iter(stem)
    });
    requests.iter().map(|p| parse_request(p)).collect()
}

/// Walk the fixture's iters in order, returning the first one whose
/// pre-iter estimate crosses `trigger`. Mirrors `mod.rs`'s F1 trigger
/// check exactly — same call, same threshold semantics.
fn first_trip_iter<'a>(
    iters: &'a [IterState],
    trigger: u32,
    system: &str,
    tools: &[crate::llm::ToolSpec],
) -> Option<&'a IterState> {
    iters.iter().find(|it| {
        let tokens = estimate_context_tokens(
            system,
            None,
            &it.messages,
            &it.tool_dialog,
            tools,
        );
        tokens >= trigger
    })
}

/// Single-turn-burst replay: one turn worth of iters whose tool dialog
/// grows monotonically. With a 80 000-token trigger, the estimator
/// should cross threshold somewhere in the middle of the burst (M2.5
/// calibration: iter 16 lands at ≈83 k). Compaction should fold the
/// early tool dialog and bring the post-fold size well below the
/// threshold so the turn can continue.
#[test]
fn replay_single_turn_burst_triggers_within_turn() {
    let iters = load_fixture_iters("compaction-single-turn-burst");
    assert!(iters.len() >= 20, "fixture should have ≥20 iters");

    // Pin tool surface to the fixture's recording-era set (see the
    // sibling multi-turn test for the rationale).
    let live = tools::specs(
        &std::sync::Arc::new(crate::skills::SkillRegistry::default()),
        &std::sync::Arc::new(crate::agents::AgentRegistry::default()),
    );
    let recorded_set = [
        "web_fetch",
        "update_plan",
        "corpus_search",
        "corpus_read",
        "task",
    ];
    let tool_specs: Vec<crate::llm::ToolSpec> = live
        .into_iter()
        .filter(|s| recorded_set.contains(&s.name.as_str()))
        .collect();
    let system = &iters[0].instructions;
    let trigger: u32 = 80_000;

    let tripped = first_trip_iter(&iters, trigger, system, &tool_specs)
        .expect("trigger must fire somewhere in the burst");

    // Sanity: not the very first iter (proves it's not "always fires").
    assert!(
        tripped.iter > 5,
        "burst should not trip at iter {} — too early",
        tripped.iter
    );
    // And it should fire in the early-to-middle of the burst, not the
    // tail: replay's job is to verify the *trigger*, not the runaway.
    assert!(
        tripped.iter < 25,
        "burst should trip well before the tail (iter {})",
        tripped.iter
    );

    // Replay the compaction fold the way production would.
    let mut summary: Option<String> = None;
    let mut messages = tripped.messages.clone();
    let mut tool_dialog = tripped.tool_dialog.clone();

    let pre_size =
        estimate_context_tokens(system, None, &messages, &tool_dialog, &tool_specs);
    assert!(pre_size >= trigger);

    let pre_msg_count = messages.len();
    let pre_dialog_count = tool_dialog.len();

    let ctx = TurnContext {
        summary: &mut summary,
        messages: &mut messages,
        tool_dialog: &mut tool_dialog,
    };
    let plan = plan_split_for(&ctx).expect("planner returns work for a tripped iter");
    let mock_summary = "[mock-compaction] folded the early turn context.".to_string();
    let post_size = apply_compaction(ctx, plan, mock_summary.clone(), system, &tool_specs);

    // Summary must be set and equal to the mock.
    assert_eq!(summary.as_deref(), Some(mock_summary.as_str()));
    // Compaction must have actually shrunk something.
    assert!(messages.len() < pre_msg_count || tool_dialog.len() < pre_dialog_count);
    // The post-fold size must be below the trigger by a real margin —
    // otherwise the next iter would re-fire immediately.
    assert!(
        post_size < pre_size,
        "post-fold size {post_size} should be smaller than pre {pre_size}"
    );
    assert!(
        post_size < trigger,
        "post-fold size {post_size} must drop under trigger {trigger}"
    );
}

/// Multi-turn-accumulation replay: 30 turns whose chat history grows
/// monotonically. The fixture also carries within-turn spikes
/// (`corpus_read` returns are tens of kilobytes apiece), so to isolate
/// the *cross-turn* accumulation signal this test only inspects the
/// START of each turn — `iter == 1`, where `tool_dialog` is empty and
/// the estimator measures pure chat-history growth. From M2.5
/// calibration the start-of-turn estimator climbs ~4 500 → ~11 000
/// across 30 turns; an 8 000-token trigger crosses around turn 20.
#[test]
fn replay_multi_turn_accumulation_triggers_at_turn_n() {
    let iters = load_fixture_iters("compaction-multi-turn-accumulation");
    assert!(iters.len() >= 60, "fixture should have ≥60 iters");

    // The fixture was recorded under M2.5's tool surface (web_fetch,
    // update_plan, corpus_search, corpus_read, task). Later milestones
    // (M3) added more tools; including their descriptions would inflate
    // the per-turn token budget against an M2.5-vintage system prompt
    // and trip the estimator prematurely. Pin the test set to the
    // recording-era list so this stays a regression test of the
    // *trigger*, not of subsequent tool-surface growth.
    let live = tools::specs(
        &std::sync::Arc::new(crate::skills::SkillRegistry::default()),
        &std::sync::Arc::new(crate::agents::AgentRegistry::default()),
    );
    let recorded_set = [
        "web_fetch",
        "update_plan",
        "corpus_search",
        "corpus_read",
        "task",
    ];
    let tool_specs: Vec<crate::llm::ToolSpec> = live
        .into_iter()
        .filter(|s| recorded_set.contains(&s.name.as_str()))
        .collect();
    let system = &iters[0].instructions;
    let trigger: u32 = 8_000;

    // Only start-of-turn snapshots — chat-history accumulation only.
    let turn_starts: Vec<&IterState> = iters.iter().filter(|it| it.iter == 1).collect();
    assert!(
        turn_starts.len() >= 25,
        "fixture should have a turn start for nearly every turn"
    );

    let tripped = first_trip_iter_filtered(&turn_starts, trigger, system, &tool_specs)
        .expect("trigger must fire at some turn-start in the multi-turn fixture");

    // The first turns have tiny chat history and must NOT trip — that
    // would mean the estimator is wildly over.
    assert!(
        tripped.turn > 10,
        "multi-turn should not trip at turn {} — too early",
        tripped.turn
    );
    // And it should trip somewhere mid-late conversation, not the last
    // turn (where the fixture happens to end).
    assert!(
        tripped.turn < 30,
        "multi-turn should trip before the last turn (got {})",
        tripped.turn
    );
    // Tool dialog must be empty at a turn start.
    assert!(
        tripped.tool_dialog.is_empty(),
        "turn-start replay should have empty tool dialog"
    );

    // Replay the fold.
    let mut summary: Option<String> = None;
    let mut messages = tripped.messages.clone();
    let mut tool_dialog = tripped.tool_dialog.clone();

    let pre_size =
        estimate_context_tokens(system, None, &messages, &tool_dialog, &tool_specs);
    assert!(pre_size >= trigger);

    let pre_msg_count = messages.len();

    let ctx = TurnContext {
        summary: &mut summary,
        messages: &mut messages,
        tool_dialog: &mut tool_dialog,
    };
    let plan = plan_split_for(&ctx).expect("planner returns work for a tripped iter");
    assert!(
        plan.early_msgs > 0,
        "in a deep multi-turn replay the planner must fold messages"
    );

    let mock_summary = format!(
        "[mock-compaction] folded {} early messages from turn {} iter {}.",
        plan.early_msgs, tripped.turn, tripped.iter
    );
    let post_size = apply_compaction(ctx, plan, mock_summary.clone(), system, &tool_specs);

    assert_eq!(summary.as_deref(), Some(mock_summary.as_str()));
    assert!(
        messages.len() < pre_msg_count,
        "multi-turn fold must drop messages (pre {pre_msg_count}, post {})",
        messages.len()
    );
    // Trailing tail is preserved by the planner — KEEP_MESSAGES = 4.
    assert!(
        messages.len() <= 4,
        "post-fold tail should be the recent-message window"
    );
    assert!(
        post_size < pre_size,
        "post-fold size {post_size} must be smaller than pre {pre_size}"
    );
    assert!(
        post_size < trigger,
        "post-fold size {post_size} must drop under trigger {trigger}"
    );
}

/// Sibling of `first_trip_iter` that takes a borrowed slice (used for
/// the turn-start subset replay above).
fn first_trip_iter_filtered<'a>(
    iters: &'a [&'a IterState],
    trigger: u32,
    system: &str,
    tools: &[crate::llm::ToolSpec],
) -> Option<&'a IterState> {
    iters
        .iter()
        .copied()
        .find(|it| {
            let tokens = estimate_context_tokens(
                system,
                None,
                &it.messages,
                &it.tool_dialog,
                tools,
            );
            tokens >= trigger
        })
}
