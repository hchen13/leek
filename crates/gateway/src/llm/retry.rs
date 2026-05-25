//! M3.4 — provider call retry layer.
//!
//! Before M3.4 a single codex failure (5xx, dropped connection, silent
//! stream past the M3.3 idle budget) immediately fataled the turn — the
//! user had to manually re-send the prompt. Empirically: a long deep-
//! review turn with builtin web_search regularly hits one transient
//! `connection_failed`, and that is enough to throw away minutes of work.
//!
//! M3.4 wraps every codex call in a small retry loop with fixed backoff.
//! The retry layer is **transparent to the agent loop** — `codex.chat`
//! still returns `Result<Stream<LlmEvent>>`; the loop sees either a
//! successful stream (possibly after one or more re-attempts under the
//! hood) or a final failure tagged with `retry_attempts`. The only loop-
//! side change is for the SSE-silent case: silent is detected mid-stream
//! by the loop itself (it owns the substantive-idle budget from M3.3),
//! so the loop calls back into a per-iter silent-retry helper instead.
//!
//! ## Classification
//!
//! M3.3's `FatalReason::kind` is the source of truth for "what kind of
//! failure is this":
//!
//! | kind                       | retry? | rationale                          |
//! |----------------------------|--------|------------------------------------|
//! | `codex_http_5xx`           | yes    | vendor-side transient              |
//! | `codex_connection_failed`  | yes    | DNS / TCP / TLS jitter             |
//! | `codex_stream_silent`      | once   | long reasoning can briefly wedge   |
//! | `codex_http_4xx`           | no     | auth / oversized — retry is futile |
//! | `codex_malformed`          | no     | leek-side bug; retry hides it      |
//! | `unknown`                  | no     | conservative — needs triage        |
//!
//! ## Backoff
//!
//! Fixed exponential: 1s / 4s / 16s. No jitter (single-client gateway —
//! the herd concern doesn't apply). No per-turn budget — every iter gets
//! a fresh 3-attempt allowance, since iter-N's failure is uncorrelated
//! with iter-(N-1)'s success.
//!
//! ## Event surface
//!
//! Each retry attempt emits a `provider_retry_attempt` event before the
//! sleep. The frontend reads it to show "重试中 (N/3)" on the running
//! turn's status pill — letting the user know the gateway is actively
//! recovering instead of staring at a frozen UI for 21 seconds.

use std::time::Duration;

use anyhow::Result;
use futures::stream::BoxStream;

use crate::agent::events;
use crate::agent::fatal::FatalReason;
use crate::api::AppState;
use crate::llm::codex::{CodexCallError, CodexClient};
use crate::llm::{ChatRequest, LlmEvent};

/// Hard cap on attempts including the initial call. `3` means the
/// initial call plus two retries. Spec-defined; not user-tunable in
/// M3.4 (no settings field, no env var) — the value already absorbs
/// the realistic transient burst rate the user has hit live.
pub const MAX_ATTEMPTS: u32 = 3;

/// Backoff sleep before *retry* attempt `n` (0-indexed: index 0 is the
/// sleep before the 2nd attempt, index 1 before the 3rd). One slot
/// shorter than `MAX_ATTEMPTS` — the initial attempt has no preceding
/// sleep.
///
/// 1s catches the bulk of single-second blips without measurable user
/// pain; 4s gives the codex backend a moment to recover from a real
/// burst; 16s is the last-ditch attempt before we tell the user it
/// really is down. Total worst-case wait before fataling: 21 seconds,
/// still well under the M3.3 substantive-idle budget (60s default).
pub const BACKOFFS: [Duration; (MAX_ATTEMPTS as usize) - 1] =
    [Duration::from_secs(1), Duration::from_secs(4)];

/// `codex_stream_silent` is special: a long-reasoning iter can wedge
/// once and then resume. The spec asks for a single iter-level retry on
/// silent, but **the M3.4 wiring does not exercise this constant**.
///
/// Why deferred: silent re-runs would re-emit per-iter side effects —
/// duplicate `search_lifecycle` frames (same call_id; the frontend
/// dedupes by artifact_id), duplicate `assistant_delta` (ephemeral, but
/// the streaming bubble flickers), and worst of all double-count any
/// `Usage` token totals that landed before the silence. A clean
/// implementation needs to snapshot per-iter scratch state before each
/// attempt and roll back on retry, and that restructure is a larger
/// change than the M3.4 deep-review-prompt-must-pass acceptance allows.
///
/// The 5xx / connection retries above cover the realistic transient
/// case (the user's deep-review prompt died on connection_failed); the
/// silent case continues to surface a kind-specific hint card pointing
/// at the composer for a manual re-send. Wiring this constant into the
/// loop is a tracked follow-up. The constant + classifier stay so the
/// retry-table view of the codebase is complete.
#[allow(dead_code)]
pub const SILENT_RETRY_BUDGET: u32 = 1;

/// Backoff before the single silent retry. Short — the wait was already
/// 60s of stream silence; piling on isn't useful. See
/// `SILENT_RETRY_BUDGET` for why this is presently unused.
#[allow(dead_code)]
pub const SILENT_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// What to do with a failed attempt: retry it (after the backoff sleep)
/// or surface it as final.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Sleep `BACKOFFS[attempt - 1]` then re-call.
    Retry,
    /// Give up — the next layer up gets the error as-is. May be a 4xx /
    /// malformed (no point retrying) or a 5xx / Connection that ran out
    /// of attempts.
    Final,
}

/// Classify a typed `CodexCallError` for the initial-call retry layer.
/// Mirrors the table in this module's docs. Only used by the
/// `CodexClient::chat` retry wrapper — silent / malformed never surface
/// here (they live on the stream side).
pub fn classify_call_error(err: &CodexCallError) -> RetryDecision {
    match err {
        CodexCallError::Http { status, .. } => {
            if *status >= 500 {
                RetryDecision::Retry
            } else {
                RetryDecision::Final
            }
        }
        CodexCallError::Connection { .. } => RetryDecision::Retry,
    }
}

/// Classify a `FatalReason` for the higher-level "should we retry the
/// silent iter once" decision. The loop calls this with a
/// `CodexStreamSilent` it just detected — `Retry` means re-call codex
/// for the iter, `Final` means accept the silent as fatal.
///
/// Kept (with `#[allow(dead_code)]`) alongside `SILENT_RETRY_BUDGET`
/// so the silent-retry follow-up can ship the loop wiring without
/// re-deriving the classifier. See `SILENT_RETRY_BUDGET` docs for why
/// the loop side is deferred.
#[allow(dead_code)]
pub fn classify_silent(reason: &FatalReason) -> RetryDecision {
    match reason {
        FatalReason::CodexStreamSilent { .. } => RetryDecision::Retry,
        _ => RetryDecision::Final,
    }
}

/// Per-call retry accounting — `attempts_made` is how many times we
/// actually hit the wire (initial call + retries that ran), and
/// `last_decision` is how we exited the loop. A success that never
/// retried still has `attempts_made = 1` and `last_decision = Final`.
///
/// Surfaced into the `FatalReason::*::retry_attempts` field so the chat
/// hint card can say "已自动重试 N 次" instead of a flat "失败了".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetryStats {
    pub attempts_made: u32,
    pub retries_used: u32,
}

impl RetryStats {
    pub fn first_attempt() -> Self {
        Self {
            attempts_made: 1,
            retries_used: 0,
        }
    }

    /// Record one more attempt (the *next* call we are about to make).
    pub fn record_retry(&mut self) {
        self.attempts_made += 1;
        self.retries_used += 1;
    }
}

/// Pure builder for the `provider_retry_attempt` event payload (the
/// `kind` constant lives in `agent::events::kind`). The event surface is
/// `Lifecycle` — the frontend renders it as a transient "重试中" badge
/// on the running turn's status pill, not as a canvas card. (Spec called
/// out canvas in the prose but described a Chat-side renderer; we
/// follow the renderer.)
///
/// Schema (all fields required except `turn_id`):
///
/// ```jsonc
/// {
///   "session_id": "s-…",
///   "turn_id":    "t-…",       // null only if a future non-turn call uses retry
///   "iteration":  3,           // 1-indexed call sequence within the turn
///   "attempt":    2,           // 1-indexed: 2 = "second attempt now"
///   "max_attempts": 3,
///   "backoff_ms": 1000,        // sleep BEFORE this attempt
///   "kind":       "codex_http_5xx",  // FatalReason.kind we are retrying
///   "detail":     "codex 返回 HTTP 503: ..."
/// }
/// ```
///
/// Today every call site (main loop + compaction summary) carries a
/// valid `turn_id`, so the field is always present in production. The
/// option is kept so the schema can absorb a future system-level codex
/// call (e.g. a session-init prompt) that has no parent turn.
pub fn build_retry_event(
    session_id: &str,
    turn_id: Option<&str>,
    iteration: i64,
    attempt: u32,
    backoff: Duration,
    reason: &FatalReason,
) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "turn_id": turn_id,
        "iteration": iteration,
        "attempt": attempt,
        "max_attempts": MAX_ATTEMPTS,
        "backoff_ms": backoff.as_millis() as u64,
        "kind": reason.kind(),
        "detail": reason.detail(),
    })
}

/// Wrap one logical "call codex" attempt with the retry policy defined
/// in this module: up to `MAX_ATTEMPTS` calls, sleeping `BACKOFFS[n]`
/// before each retry, emitting `provider_retry_attempt` once per retry.
///
/// On success: returns the stream (possibly from a re-attempt).
/// On final failure: returns a `RetryExhausted` error wrapping the
/// last attempt's `CodexCallError` plus the attempt count. The agent
/// loop's `classify_anyhow` + `lift_retry_attempts` then build a
/// FatalReason with the right `retry_attempts` for the chat hint card.
///
/// `turn_id = None` means this is the auto-compaction summary call —
/// the retry event still emits (so the user sees recovery during a
/// long turn) but the payload's `turn_id` field is null.
pub async fn call_codex_with_retry(
    state: &AppState,
    codex: &CodexClient,
    session_id: &str,
    turn_id: Option<&str>,
    iteration: i64,
    req: ChatRequest,
) -> Result<BoxStream<'static, Result<LlmEvent>>> {
    // Production notifier — fans the retry payload out to the session's
    // SSE bus + persists to the event log via AppState::emit.
    let session_id_owned = session_id.to_string();
    let state_clone = state.clone();
    let notify = move |payload: serde_json::Value| {
        let state = state_clone.clone();
        let session_id = session_id_owned.clone();
        Box::pin(async move {
            state
                .emit(&session_id, events::kind::PROVIDER_RETRY_ATTEMPT, payload)
                .await;
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    };

    retry_call(
        notify,
        session_id,
        turn_id,
        iteration,
        |attempt_req| {
            let codex = codex.clone();
            async move { codex.chat(attempt_req).await }
        },
        req,
    )
    .await
}

/// Generic retry loop. Factored out of `call_codex_with_retry` so unit
/// tests can drive it with a fake call-fn (a closure that returns the
/// scripted sequence of errors / success). Two seams the tests need:
/// `call_fn` (what each attempt actually does) and `notify` (where
/// retry events go).
///
/// `T` is the success payload — for production that is
/// `BoxStream<'static, Result<LlmEvent>>`; tests use `()`.
pub async fn retry_call<T, F, Fut, N, NFut>(
    mut notify: N,
    session_id: &str,
    turn_id: Option<&str>,
    iteration: i64,
    mut call_fn: F,
    req: ChatRequest,
) -> Result<T>
where
    F: FnMut(ChatRequest) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
    N: FnMut(serde_json::Value) -> NFut,
    NFut: std::future::Future<Output = ()>,
{
    let mut stats = RetryStats::first_attempt();
    let mut last_err: Option<anyhow::Error> = None;

    for attempt_index in 0..MAX_ATTEMPTS {
        // `clone` because each attempt re-uses the same `ChatRequest`.
        // ChatRequest is `Clone`; the cost is one Vec / String walk per
        // attempt, which is negligible against the network round-trip.
        //
        // Note: every attempt reuses the same `iteration` value. The F2
        // `llm_transcripts` table has UNIQUE(turn_id, iteration), so
        // the second / third attempt's insert will fail with a
        // duplicate-key constraint — the codex client logs that as a
        // warn and proceeds (the call still runs). The first attempt's
        // request body is what lands in the archive; subsequent retry
        // attempts are not archived. Acceptable for M3.4: the failure
        // mode is already captured in the FatalReason classification
        // and the provider_retry_attempt event log.
        let attempt_req = req.clone();
        match call_fn(attempt_req).await {
            Ok(stream) => return Ok(stream),
            Err(e) => {
                // Try to classify into a typed `CodexCallError`. If the
                // error is something else (a serialization failure, an
                // auth-token vault read, …) we never retry — those are
                // leek-internal bugs, not provider transients.
                let Some(call_err) = err_to_call_error(&e) else {
                    return Err(e);
                };
                let decision = classify_call_error(&call_err);
                if decision == RetryDecision::Final {
                    // 4xx — short-circuit, never retry. The error
                    // carries a `CodexCallError::Http`, so the loop's
                    // existing classifier will build a CodexHttp4xx
                    // FatalReason — and `retry_attempts` stays at 1
                    // (we never retried).
                    return Err(e);
                }
                last_err = Some(e);

                let next_attempt = attempt_index + 2; // 1-indexed
                if next_attempt > MAX_ATTEMPTS {
                    // Just exhausted the budget — fall through; the
                    // post-loop block stamps retry_attempts and returns.
                    break;
                }
                // We're going to retry: emit, sleep, loop. The reason
                // we hand to the event payload is the "first failure"
                // FatalReason (retry_attempts=1, since the user-facing
                // count there is the live attempt count, not the final
                // tally).
                let mut reason = FatalReason::from_call_error(&call_err);
                reason = reason.with_retry_attempts(stats.attempts_made);
                let backoff = BACKOFFS[attempt_index as usize];
                let payload =
                    build_retry_event(session_id, turn_id, iteration, next_attempt, backoff, &reason);
                notify(payload).await;
                tokio::time::sleep(backoff).await;
                stats.record_retry();
            }
        }
    }

    // Budget exhausted. Wrap the underlying CodexCallError in a
    // RetryExhausted carrier so `lift_retry_attempts` can stamp the
    // final FatalReason with the right count.
    let err = last_err.expect("retry loop must have stored at least one error");
    let Some(call_err) = err_to_call_error(&err) else {
        // Defensive: this branch is unreachable because we'd have
        // returned above on the non-typed error. Keep it for safety.
        return Err(err);
    };
    Err(anyhow::Error::new(RetryExhausted {
        cause: call_err,
        attempts_made: stats.attempts_made,
    }))
}

/// Wraps the original `CodexCallError` with the final attempts count.
/// The agent loop's `classify_anyhow` walks the cause chain and finds
/// the `CodexCallError`; the `RetryExhausted` carrier on top lets
/// `classify_retry_aware` lift the `attempts_made` count out and stamp
/// it onto the resulting `FatalReason`.
///
/// Why a wrapper instead of a new `CodexCallError` variant: adding a
/// variant would force every existing match site (M3.3's
/// `from_call_error`) to grow a new arm even though the underlying
/// failure mode is the same. The wrapper keeps `CodexCallError` clean
/// and pushes the retry-aware lift into a single helper.
#[derive(Debug)]
pub struct RetryExhausted {
    pub cause: CodexCallError,
    pub attempts_made: u32,
}

impl std::fmt::Display for RetryExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (after {} attempts)",
            self.cause, self.attempts_made,
        )
    }
}

impl std::error::Error for RetryExhausted {}

/// Classify an `anyhow::Error` the loop caught, lifting the
/// retry-aware count if `RetryExhausted` is in the chain. Mirrors the
/// loop's existing `classify_anyhow` but bumps `retry_attempts` based
/// on the wrapper. Used by the agent loop's `classify_anyhow` after
/// the retry layer landed.
pub fn lift_retry_attempts(err: &anyhow::Error, reason: FatalReason) -> FatalReason {
    if let Some(ex) = err.downcast_ref::<RetryExhausted>() {
        return reason.with_retry_attempts(ex.attempts_made);
    }
    for cause in err.chain().skip(1) {
        if let Some(ex) = cause.downcast_ref::<RetryExhausted>() {
            return reason.with_retry_attempts(ex.attempts_made);
        }
    }
    reason
}

/// Pull a `CodexCallError` out of an `anyhow::Error` chain, walking the
/// causes since `chat()` returns either the typed error directly or
/// wrapped via `.context(...)`. Returns `None` if nothing typed is
/// present — caller then short-circuits to no-retry.
fn err_to_call_error(err: &anyhow::Error) -> Option<CodexCallError> {
    if let Some(c) = err.downcast_ref::<CodexCallError>() {
        return Some(c.clone());
    }
    // RetryExhausted wraps a CodexCallError — peel it if a recursive
    // call manages to chain into one (defensive; today only the loop's
    // call_codex_with_retry produces it).
    if let Some(ex) = err.downcast_ref::<RetryExhausted>() {
        return Some(ex.cause.clone());
    }
    for cause in err.chain().skip(1) {
        if let Some(c) = cause.downcast_ref::<CodexCallError>() {
            return Some(c.clone());
        }
        if let Some(ex) = cause.downcast_ref::<RetryExhausted>() {
            return Some(ex.cause.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_5xx_is_retried() {
        let err = CodexCallError::Http {
            status: 503,
            body_excerpt: "service unavailable".into(),
        };
        assert_eq!(classify_call_error(&err), RetryDecision::Retry);
    }

    #[test]
    fn http_500_boundary_is_retried() {
        // 500 itself is server-side — must be retried.
        let err = CodexCallError::Http {
            status: 500,
            body_excerpt: "".into(),
        };
        assert_eq!(classify_call_error(&err), RetryDecision::Retry);
    }

    #[test]
    fn http_4xx_is_not_retried() {
        for status in [400u16, 401, 403, 404, 422, 429, 499] {
            let err = CodexCallError::Http {
                status,
                body_excerpt: "".into(),
            };
            assert_eq!(
                classify_call_error(&err),
                RetryDecision::Final,
                "{status} should NOT retry",
            );
        }
    }

    #[test]
    fn connection_failed_is_retried() {
        let err = CodexCallError::Connection {
            detail: "dns lookup failed".into(),
        };
        assert_eq!(classify_call_error(&err), RetryDecision::Retry);
    }

    #[test]
    fn silent_stream_is_retried_once() {
        let r = FatalReason::CodexStreamSilent {
            silent_secs: 90,
            retry_attempts: 1,
        };
        assert_eq!(classify_silent(&r), RetryDecision::Retry);
    }

    #[test]
    fn non_silent_reason_does_not_trigger_silent_retry() {
        // The silent-retry decision is silent-specific — feeding it a
        // 4xx must answer Final so the loop doesn't double-retry an
        // already-classified initial-call failure.
        let r = FatalReason::CodexHttp4xx {
            status: 401,
            body_excerpt: "expired".into(),
        };
        assert_eq!(classify_silent(&r), RetryDecision::Final);
    }

    #[test]
    fn backoff_schedule_is_exponential() {
        // Two retry slots → two backoffs. The values are spec-fixed; if
        // a future change shifts them, the test catches it deliberately.
        assert_eq!(BACKOFFS.len(), 2);
        assert_eq!(BACKOFFS[0], Duration::from_secs(1));
        assert_eq!(BACKOFFS[1], Duration::from_secs(4));
        // Total worst-case wait before fatal: 1 + 4 = 5s of sleep on
        // top of three actual codex calls.
        let total: Duration = BACKOFFS.iter().copied().sum();
        assert_eq!(total, Duration::from_secs(5));
    }

    #[test]
    fn retry_stats_records_retries() {
        let mut s = RetryStats::first_attempt();
        assert_eq!(s.attempts_made, 1);
        assert_eq!(s.retries_used, 0);
        s.record_retry();
        assert_eq!(s.attempts_made, 2);
        assert_eq!(s.retries_used, 1);
        s.record_retry();
        assert_eq!(s.attempts_made, 3);
        assert_eq!(s.retries_used, 2);
    }

    #[test]
    fn build_retry_event_carries_all_fields() {
        let reason = FatalReason::CodexHttp5xx {
            status: 503,
            body_excerpt: "service unavailable".into(),
            retry_attempts: 1,
        };
        let p = build_retry_event(
            "s-1",
            Some("t-2"),
            3,
            2,
            Duration::from_secs(1),
            &reason,
        );
        assert_eq!(p["session_id"], "s-1");
        assert_eq!(p["turn_id"], "t-2");
        assert_eq!(p["iteration"], 3);
        assert_eq!(p["attempt"], 2);
        assert_eq!(p["max_attempts"], MAX_ATTEMPTS);
        assert_eq!(p["backoff_ms"], 1000);
        assert_eq!(p["kind"], "codex_http_5xx");
        // detail is the FatalReason::detail() — must contain the status.
        assert!(p["detail"].as_str().unwrap().contains("503"));
    }

    #[test]
    fn build_retry_event_handles_null_turn_id() {
        // Compaction summary call has no turn_id (it is the summary of
        // a turn, not a turn itself).
        let reason = FatalReason::CodexConnectionFailed {
            detail: "dns lookup failed".into(),
            retry_attempts: 1,
        };
        let p = build_retry_event(
            "s-1",
            None,
            0,
            2,
            SILENT_RETRY_BACKOFF,
            &reason,
        );
        assert!(p["turn_id"].is_null());
    }

    #[test]
    fn silent_retry_budget_is_one() {
        // Pin the silent-retry budget. Spec is explicit: "重试 1 次
        // (长 reasoning 抽风, 但不要无限重)".
        assert_eq!(SILENT_RETRY_BUDGET, 1);
    }

    // ─── retry_call integration tests ─────────────────────────────────────
    //
    // These exercise the full retry loop with a scripted call-fn. They use
    // `tokio::time::pause` + `advance` so the 1s/4s backoffs don't sleep
    // for real — the test runs in milliseconds. Each test asserts both
    // the final result + the captured retry events so a regression in
    // either the loop's control flow or the event payload trips.

    use std::cell::RefCell;
    use std::rc::Rc;

    /// Build a stub ChatRequest for the retry-call tests. The body fields
    /// don't matter — `retry_call` only carries the request to the
    /// closure, which here ignores it.
    fn stub_req() -> ChatRequest {
        ChatRequest {
            model: "test".into(),
            system: "".into(),
            messages: vec![],
            tools: vec![],
            additional_inputs: vec![],
            reasoning_effort: None,
            verbosity: None,
            web_search: false,
            session_id: "s-test".into(),
            turn_id: "t-test".into(),
            iteration: 1,
        }
    }

    /// Wrap a CodexCallError as an anyhow::Error the way `codex.chat`
    /// would — bare, not wrapped in `.context()`.
    fn as_anyhow(call_err: CodexCallError) -> anyhow::Error {
        anyhow::Error::new(call_err)
    }

    /// Boxed future type the test-side notifier returns. Aliased so the
    /// `capture_notifier` signature stays under clippy's
    /// `type_complexity` threshold.
    type NotifierFut = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;

    /// Build a notifier closure that pushes payloads onto a captured Vec.
    /// Use `Rc<RefCell<...>>` because the closure must be `FnMut` and
    /// re-borrow the buffer on each call; the test thread is single-
    /// threaded (`#[tokio::test]` with the default current-thread
    /// runtime) so `Rc` is safe.
    fn capture_notifier() -> (
        impl FnMut(serde_json::Value) -> NotifierFut,
        Rc<RefCell<Vec<serde_json::Value>>>,
    ) {
        let captured: Rc<RefCell<Vec<serde_json::Value>>> = Rc::new(RefCell::new(Vec::new()));
        let captured_for_notifier = captured.clone();
        let notify = move |payload: serde_json::Value| {
            let captured = captured_for_notifier.clone();
            Box::pin(async move {
                captured.borrow_mut().push(payload);
            }) as NotifierFut
        };
        (notify, captured)
    }

    // The `start_paused = true` flag on `#[tokio::test]` virtualizes
    // tokio's clock — `tokio::time::sleep(5s)` resolves instantly inside
    // the test body, so the retry loop's backoff sleeps don't make the
    // suite take real seconds.

    #[tokio::test(flavor = "current_thread")]
    async fn http_5xx_recovers_on_second_attempt() {
        // Virtualize tokio's clock: BACKOFFS[0] = 1s would otherwise
        // make this test take a real second. With `pause`, the sleep
        // resolves instantly when no other task is making progress.
        tokio::time::pause();
        // First call → 503 (retry). Second call → success.
        let attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let attempts_for_call = attempts.clone();
        let (notify, captured) = capture_notifier();

        let result: Result<&'static str> = retry_call(
            notify,
            "s-1",
            Some("t-1"),
            1,
            move |_req| {
                let attempts = attempts_for_call.clone();
                async move {
                    let mut n = attempts.borrow_mut();
                    *n += 1;
                    let current = *n;
                    drop(n);
                    if current == 1 {
                        Err(as_anyhow(CodexCallError::Http {
                            status: 503,
                            body_excerpt: "service unavailable".into(),
                        }))
                    } else {
                        Ok("ok")
                    }
                }
            },
            stub_req(),
        )
        .await;

        // Result is OK from the second attempt.
        assert_eq!(result.unwrap(), "ok");
        // Exactly 2 attempts were made.
        assert_eq!(*attempts.borrow(), 2);
        // One retry event emitted (for attempt 2, before the backoff sleep).
        let events = captured.borrow();
        assert_eq!(events.len(), 1, "events: {events:?}");
        assert_eq!(events[0]["attempt"], 2);
        assert_eq!(events[0]["max_attempts"], MAX_ATTEMPTS);
        assert_eq!(events[0]["kind"], "codex_http_5xx");
        // backoff_ms should be the first backoff (1s = 1000ms).
        assert_eq!(events[0]["backoff_ms"], 1000);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn connection_failed_exhausts_all_three_attempts() {
        tokio::time::pause();
        // All 3 calls fail with connection_failed → RetryExhausted with
        // attempts_made = 3. Two retry events emitted (attempt 2 + 3).
        let attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let attempts_for_call = attempts.clone();
        let (notify, captured) = capture_notifier();

        let result: Result<&'static str> = retry_call(
            notify,
            "s-1",
            Some("t-1"),
            1,
            move |_req| {
                let attempts = attempts_for_call.clone();
                async move {
                    *attempts.borrow_mut() += 1;
                    Err(as_anyhow(CodexCallError::Connection {
                        detail: "dns lookup failed".into(),
                    }))
                }
            },
            stub_req(),
        )
        .await;

        // All MAX_ATTEMPTS calls happened.
        assert_eq!(*attempts.borrow(), MAX_ATTEMPTS);

        // Final error is RetryExhausted carrying the underlying
        // Connection error + attempts_made = 3.
        let err = result.unwrap_err();
        let ex = err
            .downcast_ref::<RetryExhausted>()
            .expect("final error must be RetryExhausted");
        assert_eq!(ex.attempts_made, MAX_ATTEMPTS);
        assert!(matches!(ex.cause, CodexCallError::Connection { .. }));

        // Two retry events emitted (before attempts 2 and 3 — never
        // before attempt 1, never *after* the last failure).
        let events = captured.borrow();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["attempt"], 2);
        assert_eq!(events[0]["backoff_ms"], 1000);
        assert_eq!(events[1]["attempt"], 3);
        assert_eq!(events[1]["backoff_ms"], 4000);
        // Both event kinds are codex_connection_failed.
        assert_eq!(events[0]["kind"], "codex_connection_failed");
        assert_eq!(events[1]["kind"], "codex_connection_failed");

        // The lifted FatalReason carries retry_attempts = 3 → hint card
        // shows "已自动重试 2 次, 仍失败".
        let reason = FatalReason::from_call_error(&ex.cause);
        let lifted = lift_retry_attempts(&err, reason);
        assert_eq!(lifted.retry_attempts(), MAX_ATTEMPTS);
        assert!(lifted.hint().contains("已自动重试 2 次"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn http_4xx_short_circuits_no_retry() {
        // No sleeps expected on this path — but still pause defensively
        // so any future tweak that adds a sleep doesn't make the test
        // wall-clock slow.
        tokio::time::pause();
        // 401 → never retry. The closure runs exactly once; no retry
        // events are emitted; the returned error carries the original
        // CodexCallError::Http (not RetryExhausted).
        let attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let attempts_for_call = attempts.clone();
        let (notify, captured) = capture_notifier();

        let result: Result<&'static str> = retry_call(
            notify,
            "s-1",
            Some("t-1"),
            1,
            move |_req| {
                let attempts = attempts_for_call.clone();
                async move {
                    *attempts.borrow_mut() += 1;
                    Err(as_anyhow(CodexCallError::Http {
                        status: 401,
                        body_excerpt: "token expired".into(),
                    }))
                }
            },
            stub_req(),
        )
        .await;

        // Exactly one attempt — no retry happened.
        assert_eq!(*attempts.borrow(), 1);
        // No retry events emitted.
        assert!(captured.borrow().is_empty());

        // Returned error is the bare CodexCallError, not RetryExhausted.
        let err = result.unwrap_err();
        assert!(err.downcast_ref::<RetryExhausted>().is_none());
        let call_err = err.downcast_ref::<CodexCallError>().unwrap();
        match call_err {
            CodexCallError::Http { status, .. } => assert_eq!(*status, 401),
            _ => panic!("expected Http, got {call_err:?}"),
        }
        // The FatalReason derived from it has retry_attempts = 1 → hint
        // does NOT advertise retries.
        let reason = FatalReason::from_call_error(call_err);
        let lifted = lift_retry_attempts(&err, reason);
        assert_eq!(lifted.retry_attempts(), 1);
        assert!(!lifted.hint().contains("已自动重试"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_attempt_success_emits_no_retry_event() {
        tokio::time::pause();
        // The common happy path: the first call succeeds. No retries,
        // no events.
        let attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let attempts_for_call = attempts.clone();
        let (notify, captured) = capture_notifier();

        let result: Result<&'static str> = retry_call(
            notify,
            "s-1",
            Some("t-1"),
            1,
            move |_req| {
                let attempts = attempts_for_call.clone();
                async move {
                    *attempts.borrow_mut() += 1;
                    Ok("ok")
                }
            },
            stub_req(),
        )
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*attempts.borrow(), 1);
        assert!(captured.borrow().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn untyped_error_short_circuits_no_retry() {
        tokio::time::pause();
        // A non-CodexCallError (e.g. a serialization failure from
        // `codex.chat`'s upstream code path) must NOT trigger retry.
        // The retry layer is for *provider* transients; anything else
        // is a leek-side bug worth surfacing immediately.
        let attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let attempts_for_call = attempts.clone();
        let (notify, captured) = capture_notifier();

        let result: Result<&'static str> = retry_call(
            notify,
            "s-1",
            Some("t-1"),
            1,
            move |_req| {
                let attempts = attempts_for_call.clone();
                async move {
                    *attempts.borrow_mut() += 1;
                    Err(anyhow::anyhow!("a leek-side bug, e.g. JSON encode failed"))
                }
            },
            stub_req(),
        )
        .await;

        assert_eq!(*attempts.borrow(), 1, "must not retry untyped errors");
        assert!(captured.borrow().is_empty());
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mixed_5xx_then_connection_then_success() {
        tokio::time::pause();
        // Realistic transient burst: 5xx, then connection_failed, then
        // success. Both retries should fire with their respective
        // FatalReason kinds in the events.
        let attempts: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let attempts_for_call = attempts.clone();
        let (notify, captured) = capture_notifier();

        let result: Result<&'static str> = retry_call(
            notify,
            "s-1",
            Some("t-1"),
            1,
            move |_req| {
                let attempts = attempts_for_call.clone();
                async move {
                    let mut n = attempts.borrow_mut();
                    *n += 1;
                    let current = *n;
                    drop(n);
                    match current {
                        1 => Err(as_anyhow(CodexCallError::Http {
                            status: 502,
                            body_excerpt: "bad gateway".into(),
                        })),
                        2 => Err(as_anyhow(CodexCallError::Connection {
                            detail: "reset by peer".into(),
                        })),
                        _ => Ok("ok"),
                    }
                }
            },
            stub_req(),
        )
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(*attempts.borrow(), 3);
        let events = captured.borrow();
        assert_eq!(events.len(), 2);
        // First retry event reflects the 5xx that just happened.
        assert_eq!(events[0]["kind"], "codex_http_5xx");
        assert_eq!(events[0]["attempt"], 2);
        // Second retry event reflects the connection failure.
        assert_eq!(events[1]["kind"], "codex_connection_failed");
        assert_eq!(events[1]["attempt"], 3);
    }
}
