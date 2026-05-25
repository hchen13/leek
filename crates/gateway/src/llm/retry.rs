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
//! Pre-M3.5: 1s / 4s exponential, 3 total attempts (1 initial + 2 retries).
//! M3.5 widened to 1s / 5s / 15s / 30s / 60s, 5 total attempts: live network
//! bursts long enough that the 5s second slot saved a real PM-critical turn
//! (T31b 长电科技 deep dive). No jitter (single-client gateway — the herd
//! concern doesn't apply). No per-turn budget — every iter gets a fresh
//! 5-attempt allowance, since iter-N's failure is uncorrelated with
//! iter-(N-1)'s success.
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
use futures::StreamExt;

use crate::agent::events;
use crate::agent::fatal::FatalReason;
use crate::api::AppState;
use crate::llm::codex::{CodexCallError, CodexClient};
use crate::llm::{ChatRequest, LlmEvent};

/// Hard cap on attempts including the initial call. `5` means the
/// initial call plus four retries. Spec-defined (M3.5); not user-tunable
/// (no settings field, no env var) — the value reflects the live network
/// burst pattern the user has hit on PM-critical prompts. The spec says
/// "5 attempts, 4 backoff slots" — we honor the smaller of the two
/// consistent readings of the dispatch (the 5th 60s slot would only be
/// used if MAX_ATTEMPTS were 6; we stay at 5 per the literal "3 → 5").
pub const MAX_ATTEMPTS: u32 = 5;

/// Backoff sleep before *retry* attempt `n` (0-indexed: index 0 is the
/// sleep before the 2nd attempt, …, index 3 before the 5th). One slot
/// shorter than `MAX_ATTEMPTS` — the initial attempt has no preceding
/// sleep.
///
/// M3.5 widened from the M3.4 `[1s, 4s]` pair:
/// - 1s: bulk of single-second blips, no measurable user pain.
/// - 5s: real-burst recovery (codex side glitches lasting a few seconds).
/// - 15s: codex side restart / backend brownout, retry once it recovers.
/// - 30s: last-ditch for a longer wedge before we tell the user.
///
/// Total worst-case wait before fataling: 1 + 5 + 15 + 30 = 51 seconds of
/// sleep, on top of however long each attempt's request actually runs.
/// The user sees recovery progress via `provider_retry_attempt` events,
/// so 51s of total backoff is acceptable for a transient network burst.
pub const BACKOFFS: [Duration; (MAX_ATTEMPTS as usize) - 1] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];

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

/// M3.5: same wrapper as `call_codex_with_retry`, but the returned stream
/// itself retries mid-flight on a retryable SSE-layer error
/// (`CodexStreamTimeout` / `CodexStreamDecodeError` / mid-stream
/// `CodexConnectionFailed` / `CodexHttp5xx`). The initial-call retry and
/// the stream-mid retry share one budget — `MAX_ATTEMPTS` across both —
/// so a turn cannot run away with a 10-attempt sequence by failing once
/// at connect and then four times at SSE.
///
/// ### Partial-state safety
///
/// A stream-mid error can only trigger a retry if **no substantive
/// `LlmEvent` has been yielded yet from any inner stream**. "Substantive"
/// is `LlmEvent::is_substantive` — text deltas, function calls, web
/// search frames, usage, message_end. `Ping` does not block retry (the
/// codex stream emits Pings continuously during long reasoning, and the
/// T31b failure happened mid-reasoning while only Pings were flowing).
///
/// Why the gate: once any substantive event has been yielded the caller
/// (agent loop) has already acted on it — text was streamed to the chat
/// bubble, a tool call was dispatched, usage was counted. A retry would
/// re-emit the same content from a fresh stream, causing duplicate tool
/// dispatches, double-counted Usage tokens, and a flickering chat bubble.
/// We choose to surface the error as fatal in that case rather than risk
/// downstream corruption. In the realistic failure pattern (T31b
/// "reading SSE chunk → operation timed out" mid-reasoning), only Pings
/// have flowed, so the gate passes and retry runs.
///
/// ### Usage double-count
///
/// `Usage` is emitted by the codex backend as part of `response.completed`
/// — it lands at the **end** of the response, never mid-stream. A
/// successful retry produces one `Usage` event (from the new stream). A
/// failed-mid-stream attempt that the wrapper restarts produces no
/// `Usage`. Hence: no double-counting even without explicit
/// deduplication — the partial-state gate above guarantees that a stream
/// which already emitted Usage is past the retry point.
pub async fn call_codex_with_stream_retry(
    state: &AppState,
    codex: &CodexClient,
    session_id: &str,
    turn_id: Option<&str>,
    iteration: i64,
    req: ChatRequest,
) -> Result<BoxStream<'static, Result<LlmEvent>>> {
    // First, get the initial stream via the initial-call retry path (5xx /
    // connection_failed before any byte arrives). If the budget is
    // exhausted here the wrapper bails the way `call_codex_with_retry`
    // does — RetryExhausted carrying the per-call attempt count.
    let (initial_stream, attempts_used_for_initial) =
        match call_codex_with_initial_retry_counted(
            state,
            codex,
            session_id,
            turn_id,
            iteration,
            req.clone(),
        )
        .await
        {
            Ok(pair) => pair,
            Err(e) => return Err(e),
        };

    // Build the retry-wrapping stream. Production notifier (SSE bus + event
    // log) and production reconnect (`codex.chat(req)`) plug into the
    // pure state machine `stream_retry_wrap`. Tests use the same state
    // machine with scripted closures (see `tests::stream_retry_*`).
    let state_clone = state.clone();
    let codex_clone = codex.clone();
    let session_id_owned = session_id.to_string();
    let req_for_reconnect = req.clone();

    let notify = move |payload: serde_json::Value| {
        let state = state_clone.clone();
        let session = session_id_owned.clone();
        Box::pin(async move {
            state
                .emit(&session, events::kind::PROVIDER_RETRY_ATTEMPT, payload)
                .await;
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    };
    let reconnect = move || {
        let codex = codex_clone.clone();
        let req = req_for_reconnect.clone();
        Box::pin(async move { codex.chat(req).await }) as std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<BoxStream<'static, Result<LlmEvent>>>,
                    > + Send,
            >,
        >
    };

    Ok(stream_retry_wrap(
        initial_stream,
        attempts_used_for_initial,
        session_id.to_string(),
        turn_id.map(|s| s.to_string()),
        iteration,
        notify,
        reconnect,
    ))
}

/// Pure state machine for the stream-retry path — separated from the
/// production notifier / reconnect so tests can drive it with scripted
/// streams + a capture closure. Returns the wrapped stream.
///
/// `initial_attempts_used` is how many attempts the initial-call retry
/// path already consumed (1 if the very first POST succeeded; higher if
/// it retried before the SSE bytes started). The stream-mid retry shares
/// the same `MAX_ATTEMPTS` budget.
fn stream_retry_wrap<N, NFut, R, RFut>(
    initial_stream: BoxStream<'static, Result<LlmEvent>>,
    initial_attempts_used: u32,
    session_id: String,
    turn_id: Option<String>,
    iteration: i64,
    mut notify: N,
    mut reconnect: R,
) -> BoxStream<'static, Result<LlmEvent>>
where
    N: FnMut(serde_json::Value) -> NFut + Send + 'static,
    NFut: std::future::Future<Output = ()> + Send + 'static,
    R: FnMut() -> RFut + Send + 'static,
    RFut: std::future::Future<Output = Result<BoxStream<'static, Result<LlmEvent>>>>
        + Send
        + 'static,
{
    // Start the stats from however many attempts the initial call used.
    let initial_stats = {
        let mut s = RetryStats::first_attempt();
        for _ in 1..initial_attempts_used {
            s.record_retry();
        }
        s
    };

    let wrapped = async_stream::stream! {
        let mut stream = initial_stream;
        let mut stats = initial_stats;
        // Latch: true once any non-Ping event has been yielded to the
        // caller. Once true, a stream-mid error is fatal — retrying
        // would re-emit content the agent loop has already acted on
        // (duplicate tool dispatches, double-counted Usage, …).
        let mut substantive_seen = false;
        loop {
            match stream.next().await {
                Some(Ok(ev)) => {
                    if ev.is_substantive() {
                        substantive_seen = true;
                    }
                    yield Ok(ev);
                }
                Some(Err(e)) => {
                    let reason = FatalReason::from_stream_error(&e);
                    let retryable = is_stream_kind_retryable(&reason);
                    let has_budget = stats.attempts_made < MAX_ATTEMPTS;
                    if !retryable || substantive_seen || !has_budget {
                        // Surface the error to the caller. If we ran any
                        // retries, wrap it in RetryExhausted so the loop's
                        // lift_retry_attempts stamps the final count onto
                        // the FatalReason.
                        if stats.attempts_made > 1 {
                            yield Err(wrap_stream_err_with_retry(
                                &reason,
                                stats.attempts_made,
                            ));
                        } else {
                            yield Err(e);
                        }
                        return;
                    }

                    // Retry: emit event, sleep backoff, reconnect.
                    let backoff_index = (stats.attempts_made - 1) as usize;
                    let backoff = BACKOFFS[backoff_index];
                    let next_attempt = stats.attempts_made + 1; // 1-indexed
                    let stamped =
                        reason.clone().with_retry_attempts(stats.attempts_made);
                    let payload = build_retry_event(
                        &session_id,
                        turn_id.as_deref(),
                        iteration,
                        next_attempt,
                        backoff,
                        &stamped,
                    );
                    notify(payload).await;
                    tokio::time::sleep(backoff).await;
                    stats.record_retry();

                    match reconnect().await {
                        Ok(s) => {
                            stream = s;
                        }
                        Err(call_err) => {
                            yield Err(wrap_initial_call_err_with_retry(
                                call_err,
                                stats.attempts_made,
                            ));
                            return;
                        }
                    }
                }
                None => return,
            }
        }
    };

    Box::pin(wrapped)
}

/// Wrap a stream-side anyhow error in a `RetryExhausted` carrier so the
/// agent loop's `lift_retry_attempts` stamps the FatalReason with the
/// correct `attempts_made` count. The cause inside RetryExhausted is a
/// fabricated `CodexCallError::Connection` whose `detail` reflects the
/// stream-side classification (we don't actually have a CodexCallError
/// for a mid-stream timeout, but the agent loop's `classify_anyhow`
/// will land on `FatalReason::from_stream_error` via the cause chain).
fn wrap_stream_err_with_retry(reason: &FatalReason, attempts_made: u32) -> anyhow::Error {
    // We need both:
    //   (a) the original-style stream error so `classify_anyhow` →
    //       `from_stream_error` recognizes its kind (the string heuristic
    //       in classify_anyhow looks for "reading SSE chunk" etc.);
    //   (b) a `RetryExhausted` somewhere in the chain so
    //       `lift_retry_attempts` finds the count.
    //
    // We carry both by wrapping a synthetic message-shaped error with a
    // RetryExhausted carrying a fake Connection cause. The agent loop's
    // path is: classify_anyhow sees the top-level anyhow message →
    // matches "reading SSE chunk" if present → builds the correct kind;
    // lift_retry_attempts walks the chain → finds RetryExhausted → stamps
    // the count. Done.
    //
    // The message format mirrors what `parse_sse_stream` emits so
    // `from_stream_error` lands on the same kind. The cause chain has
    // RetryExhausted, then the synthetic Connection (so error_to_call_error
    // can also find it if some other path does a downcast walk).
    let detail = format!("{} (after {attempts_made} attempts)", reason.detail());
    let cause = CodexCallError::Connection { detail: detail.clone() };
    let exhausted = RetryExhausted {
        cause,
        attempts_made,
    };
    // Use the original kind's leading marker string so `from_stream_error`
    // re-classifies into the same kind (preserves "reading SSE chunk" for
    // stream_timeout / connection_failed, "non-UTF-8 in SSE event" for
    // decode_error, etc.).
    let head = stream_kind_message_head(reason);
    anyhow::Error::new(exhausted).context(format!("{head}: {detail}"))
}

/// Same as `wrap_stream_err_with_retry` but the underlying failure was a
/// fresh initial POST that failed during retry — we have a real
/// `CodexCallError` to wrap in `RetryExhausted` directly.
fn wrap_initial_call_err_with_retry(err: anyhow::Error, attempts_made: u32) -> anyhow::Error {
    let Some(call_err) = err_to_call_error(&err) else {
        return err;
    };
    anyhow::Error::new(RetryExhausted {
        cause: call_err,
        attempts_made,
    })
}

/// Re-derive the error-message prefix that `parse_sse_stream` would have
/// emitted for `reason`. Lets us re-build an error whose top-level message
/// hits the same `from_stream_error` classifier on the way back through
/// the agent loop.
fn stream_kind_message_head(reason: &FatalReason) -> &'static str {
    match reason {
        FatalReason::CodexStreamTimeout { .. } | FatalReason::CodexConnectionFailed { .. } => {
            "reading SSE chunk"
        }
        FatalReason::CodexStreamDecodeError { .. } => "non-UTF-8 in SSE event",
        _ => "stream error",
    }
}

/// Which stream-side FatalReason kinds should the wrapper restart the
/// chat call for? `CodexMalformed` is not in the set — a true malformed
/// reply is a leek-side bug and a retry would just paper over it.
/// `CodexStreamSilent` is not in the set — silent is detected by the loop
/// (idle timeout), not by the stream itself; and silent retry needs the
/// per-iter state rollback that the spec defers (see SILENT_RETRY_BUDGET).
fn is_stream_kind_retryable(reason: &FatalReason) -> bool {
    matches!(
        reason,
        FatalReason::CodexStreamTimeout { .. }
            | FatalReason::CodexStreamDecodeError { .. }
            | FatalReason::CodexConnectionFailed { .. }
            | FatalReason::CodexHttp5xx { .. }
    )
}

/// Like `call_codex_with_retry`, but also returns how many attempts the
/// initial-call retry consumed — needed by `call_codex_with_stream_retry`
/// so the stream-mid retry budget continues from where the initial-call
/// retry left off (the two share one MAX_ATTEMPTS budget).
async fn call_codex_with_initial_retry_counted(
    state: &AppState,
    codex: &CodexClient,
    session_id: &str,
    turn_id: Option<&str>,
    iteration: i64,
    req: ChatRequest,
) -> Result<(BoxStream<'static, Result<LlmEvent>>, u32)> {
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
    let attempts_used = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let attempts_used_for_call = attempts_used.clone();

    let stream = retry_call(
        notify,
        session_id,
        turn_id,
        iteration,
        |attempt_req| {
            let codex = codex.clone();
            let attempts_used = attempts_used_for_call.clone();
            async move {
                attempts_used.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                codex.chat(attempt_req).await
            }
        },
        req,
    )
    .await?;
    let used = attempts_used.load(std::sync::atomic::Ordering::Relaxed);
    Ok((stream, used))
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
        // M3.5: five attempts → four backoffs. The values are spec-fixed;
        // a future change shifts them, the test catches it deliberately.
        assert_eq!(MAX_ATTEMPTS, 5);
        assert_eq!(BACKOFFS.len(), (MAX_ATTEMPTS as usize) - 1);
        assert_eq!(BACKOFFS[0], Duration::from_secs(1));
        assert_eq!(BACKOFFS[1], Duration::from_secs(5));
        assert_eq!(BACKOFFS[2], Duration::from_secs(15));
        assert_eq!(BACKOFFS[3], Duration::from_secs(30));
        // Total worst-case wait before fatal: 51s of sleep on top of
        // five actual codex calls.
        let total: Duration = BACKOFFS.iter().copied().sum();
        assert_eq!(total, Duration::from_secs(1 + 5 + 15 + 30));
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
    async fn connection_failed_exhausts_all_attempts() {
        tokio::time::pause();
        // M3.5: all 5 calls fail with connection_failed → RetryExhausted
        // with attempts_made = MAX_ATTEMPTS. Four retry events emitted
        // (before attempts 2 / 3 / 4 / 5).
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
        // Connection error + attempts_made = MAX_ATTEMPTS.
        let err = result.unwrap_err();
        let ex = err
            .downcast_ref::<RetryExhausted>()
            .expect("final error must be RetryExhausted");
        assert_eq!(ex.attempts_made, MAX_ATTEMPTS);
        assert!(matches!(ex.cause, CodexCallError::Connection { .. }));

        // MAX_ATTEMPTS - 1 retry events emitted (one before each retry,
        // never before the initial call, never after the last failure).
        let events = captured.borrow();
        assert_eq!(events.len() as u32, MAX_ATTEMPTS - 1);
        // Walk the events: attempt 2 with BACKOFFS[0], attempt 3 with
        // BACKOFFS[1], …
        for (idx, event) in events.iter().enumerate() {
            assert_eq!(event["attempt"], (idx as u32 + 2));
            assert_eq!(
                event["backoff_ms"],
                BACKOFFS[idx].as_millis() as u64,
            );
            assert_eq!(event["kind"], "codex_connection_failed");
        }

        // The lifted FatalReason carries retry_attempts = MAX_ATTEMPTS →
        // hint shows "已自动重试 (MAX_ATTEMPTS-1) 次, 仍失败".
        let reason = FatalReason::from_call_error(&ex.cause);
        let lifted = lift_retry_attempts(&err, reason);
        assert_eq!(lifted.retry_attempts(), MAX_ATTEMPTS);
        let expected_note = format!("已自动重试 {} 次", MAX_ATTEMPTS - 1);
        assert!(
            lifted.hint().contains(&expected_note),
            "hint was: {}",
            lifted.hint(),
        );
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

    // ─── M3.5 stream_retry_wrap integration tests ─────────────────────────
    //
    // These drive `stream_retry_wrap` with synthetic streams + scripted
    // `reconnect` closures. Backoff sleeps are virtualized via
    // `tokio::time::pause`. Uses Arc<Mutex> rather than Rc<RefCell> so
    // the captures satisfy `Send + 'static` on the closure bounds.

    use std::sync::Arc;
    use std::sync::Mutex;

    /// Build a stream of `Result<LlmEvent>` from a Vec, in order.
    fn scripted_stream(
        items: Vec<Result<LlmEvent>>,
    ) -> BoxStream<'static, Result<LlmEvent>> {
        Box::pin(futures::stream::iter(items))
    }

    /// Test-side reconnect-future type — aliased so the helper signatures
    /// stay readable under clippy's `type_complexity` threshold.
    type ReconnectFut = std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<BoxStream<'static, Result<LlmEvent>>>,
                > + Send,
        >,
    >;

    /// A failing reconnect closure — always returns the given error.
    fn reconnect_always_err(
        err_msg: &'static str,
    ) -> impl FnMut() -> ReconnectFut {
        move || {
            Box::pin(async move {
                Err(anyhow::Error::new(CodexCallError::Connection {
                    detail: err_msg.to_string(),
                }))
            })
        }
    }

    /// A scripted reconnect — returns each scripted stream in sequence.
    /// Wraps the queue in Arc<Mutex> so the closure can mutate across
    /// calls. Once exhausted, returns an empty Ok stream (would normally
    /// be unreachable in tests).
    fn reconnect_with(
        scripts: Vec<Vec<Result<LlmEvent>>>,
    ) -> impl FnMut() -> ReconnectFut {
        let queue: Arc<Mutex<std::collections::VecDeque<Vec<Result<LlmEvent>>>>> =
            Arc::new(Mutex::new(scripts.into_iter().collect()));
        move || {
            let queue = queue.clone();
            Box::pin(async move {
                let next = queue.lock().unwrap().pop_front();
                Ok(match next {
                    Some(items) => scripted_stream(items),
                    None => scripted_stream(vec![]),
                })
            })
        }
    }

    /// Send-safe notifier: pushes payloads onto a shared Vec under a
    /// std Mutex. Multi-threaded runtime accommodates the closure's
    /// `Send + 'static` requirement.
    type SendNotifierFut =
        std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

    fn send_notifier() -> (
        impl FnMut(serde_json::Value) -> SendNotifierFut + Send + 'static,
        Arc<Mutex<Vec<serde_json::Value>>>,
    ) {
        let captured: Arc<Mutex<Vec<serde_json::Value>>> =
            Arc::new(Mutex::new(Vec::new()));
        let captured_for = captured.clone();
        let notify = move |payload: serde_json::Value| {
            let captured = captured_for.clone();
            Box::pin(async move {
                captured.lock().unwrap().push(payload);
            }) as SendNotifierFut
        };
        (notify, captured)
    }

    /// Helper: build a stream-side error whose top-level message hits
    /// `from_stream_error` as a timeout (the T31b crash signature).
    fn timeout_err() -> anyhow::Error {
        anyhow::anyhow!("operation timed out").context("reading SSE chunk")
    }

    /// Helper: build a stream-side error that lands as `codex_malformed`
    /// (e.g. `provider returned an error event`). Non-retryable.
    fn malformed_err() -> anyhow::Error {
        anyhow::anyhow!("provider returned an error event: bogus")
    }

    /// M3.5 spec test §A:
    /// SSE stream 中段 timeout → retry 触发,第 2 次成功.
    /// Initial stream yields a Ping then a timeout; reconnect produces a
    /// stream with a TextDelta + completion. Wrapper transparently
    /// recovers; caller sees Ping, then TextDelta, then end-of-stream.
    /// One `provider_retry_attempt` event captured.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_retry_recovers_on_second_attempt() {
        tokio::time::pause();
        let initial = scripted_stream(vec![
            Ok(LlmEvent::Ping),
            Err(timeout_err()),
        ]);
        let reconnect = reconnect_with(vec![vec![
            Ok(LlmEvent::TextDelta {
                text: "recovered".into(),
            }),
            Ok(LlmEvent::MessageEnd {
                stop_reason: crate::llm::StopReason::EndTurn,
            }),
        ]]);
        let (notify, captured) = send_notifier();

        let mut wrapped = stream_retry_wrap(
            initial,
            1,
            "s-1".into(),
            Some("t-1".into()),
            1,
            notify,
            reconnect,
        );

        let mut yielded: Vec<LlmEvent> = Vec::new();
        while let Some(item) = wrapped.next().await {
            yielded.push(item.expect("no errors should leak"));
        }
        assert_eq!(yielded.len(), 3);
        assert!(matches!(yielded[0], LlmEvent::Ping));
        match &yielded[1] {
            LlmEvent::TextDelta { text } => assert_eq!(text, "recovered"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        assert!(matches!(yielded[2], LlmEvent::MessageEnd { .. }));

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["attempt"], 2);
        assert_eq!(events[0]["kind"], "codex_stream_timeout");
        assert_eq!(events[0]["backoff_ms"], 1000);
        assert_eq!(events[0]["session_id"], "s-1");
        assert_eq!(events[0]["turn_id"], "t-1");
    }

    /// M3.5 spec test §A:
    /// SSE stream 5 次都 timeout → 最终 fatal kind=CodexStreamTimeout.
    /// Every attempt times out — initial + 4 reconnects, total MAX_ATTEMPTS.
    /// Caller sees `MAX_ATTEMPTS - 1` retry events captured, then a final
    /// Err carrying RetryExhausted with attempts_made == MAX_ATTEMPTS.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_retry_exhausts_after_max_attempts() {
        tokio::time::pause();
        let initial = scripted_stream(vec![
            Ok(LlmEvent::Ping),
            Err(timeout_err()),
        ]);
        // Four reconnects, each also a timeout right after a Ping. Total
        // attempts: 1 initial + 4 reconnect = MAX_ATTEMPTS.
        let scripts: Vec<Vec<Result<LlmEvent>>> = (0..(MAX_ATTEMPTS - 1))
            .map(|_| vec![Ok(LlmEvent::Ping), Err(timeout_err())])
            .collect();
        let reconnect = reconnect_with(scripts);
        let (notify, captured) = send_notifier();

        let mut wrapped = stream_retry_wrap(
            initial,
            1,
            "s-1".into(),
            Some("t-1".into()),
            1,
            notify,
            reconnect,
        );

        let mut errors: Vec<anyhow::Error> = Vec::new();
        let mut pings_seen = 0u32;
        while let Some(item) = wrapped.next().await {
            match item {
                Ok(LlmEvent::Ping) => pings_seen += 1,
                Ok(other) => panic!("unexpected event {other:?}"),
                Err(e) => errors.push(e),
            }
        }
        // One Ping per attempt is yielded (the script puts Ping → timeout).
        assert_eq!(pings_seen, MAX_ATTEMPTS);

        // Exactly one final error.
        assert_eq!(errors.len(), 1);
        let err = errors.remove(0);
        let exhausted = err
            .downcast_ref::<RetryExhausted>()
            .expect("final error must be RetryExhausted");
        assert_eq!(exhausted.attempts_made, MAX_ATTEMPTS);

        // The lifted FatalReason carries the stream_timeout kind + the
        // exhausted attempt count.
        let stream_reason = FatalReason::from_stream_error(&err);
        let lifted = lift_retry_attempts(&err, stream_reason);
        assert_eq!(lifted.kind(), "codex_stream_timeout");
        assert_eq!(lifted.retry_attempts(), MAX_ATTEMPTS);
        let expected_note = format!("已自动重试 {} 次", MAX_ATTEMPTS - 1);
        assert!(
            lifted.hint().contains(&expected_note),
            "hint was: {}",
            lifted.hint()
        );

        // `MAX_ATTEMPTS - 1` retry events captured: one per retry attempt.
        let events = captured.lock().unwrap();
        assert_eq!(events.len() as u32, MAX_ATTEMPTS - 1);
        for (idx, event) in events.iter().enumerate() {
            assert_eq!(event["kind"], "codex_stream_timeout");
            assert_eq!(event["attempt"], (idx as u32 + 2));
            assert_eq!(
                event["backoff_ms"],
                BACKOFFS[idx].as_millis() as u64,
            );
        }
    }

    /// Substantive event yielded BEFORE the failure → no retry, the
    /// caller sees the error as-is. This guards against the double-emit
    /// risk: once a tool call has been dispatched, retrying would re-emit
    /// it.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_retry_skipped_after_substantive_event() {
        tokio::time::pause();
        let initial = scripted_stream(vec![
            Ok(LlmEvent::Ping),
            Ok(LlmEvent::TextDelta { text: "hi".into() }),
            Err(timeout_err()),
        ]);
        let reconnect = reconnect_always_err("must not be called");
        let (notify, captured) = send_notifier();

        let mut wrapped = stream_retry_wrap(
            initial,
            1,
            "s-1".into(),
            Some("t-1".into()),
            1,
            notify,
            reconnect,
        );

        let mut events_seen: Vec<LlmEvent> = Vec::new();
        let mut final_err: Option<anyhow::Error> = None;
        while let Some(item) = wrapped.next().await {
            match item {
                Ok(ev) => events_seen.push(ev),
                Err(e) => {
                    final_err = Some(e);
                    break;
                }
            }
        }
        // Both Ping and TextDelta yielded before the failure.
        assert_eq!(events_seen.len(), 2);
        assert!(matches!(events_seen[0], LlmEvent::Ping));
        assert!(matches!(events_seen[1], LlmEvent::TextDelta { .. }));
        // The error is the raw stream error (no retry happened).
        let err = final_err.expect("must have errored");
        assert!(err.downcast_ref::<RetryExhausted>().is_none());
        // No retry events captured — we never attempted retry.
        assert!(captured.lock().unwrap().is_empty());
    }

    /// Non-retryable kind (`codex_malformed`) is surfaced immediately
    /// even with no substantive output yet.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_retry_skipped_for_malformed_kind() {
        tokio::time::pause();
        let initial = scripted_stream(vec![
            Ok(LlmEvent::Ping),
            Err(malformed_err()),
        ]);
        let reconnect = reconnect_always_err("must not be called");
        let (notify, captured) = send_notifier();

        let mut wrapped = stream_retry_wrap(
            initial,
            1,
            "s-1".into(),
            Some("t-1".into()),
            1,
            notify,
            reconnect,
        );

        let mut got_err = false;
        while let Some(item) = wrapped.next().await {
            if item.is_err() {
                got_err = true;
                break;
            }
        }
        assert!(got_err);
        assert!(captured.lock().unwrap().is_empty(), "no retries expected");
    }

    /// Decode error (`non-UTF-8 in SSE event`) is retryable — the
    /// wrapper restarts the chat call.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_retry_recovers_from_decode_error() {
        tokio::time::pause();
        let initial = scripted_stream(vec![Err(anyhow::anyhow!(
            "non-UTF-8 in SSE event"
        ))]);
        let reconnect = reconnect_with(vec![vec![Ok(LlmEvent::TextDelta {
            text: "ok".into(),
        })]]);
        let (notify, captured) = send_notifier();

        let mut wrapped = stream_retry_wrap(
            initial,
            1,
            "s-1".into(),
            None, // compaction-summary style call (turn_id null)
            7,
            notify,
            reconnect,
        );

        let mut deltas = 0;
        while let Some(item) = wrapped.next().await {
            if matches!(item.unwrap(), LlmEvent::TextDelta { .. }) {
                deltas += 1;
            }
        }
        assert_eq!(deltas, 1);
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["kind"], "codex_stream_decode_error");
        // turn_id absent (null) — compaction path.
        assert!(events[0]["turn_id"].is_null());
    }

    /// Verify that `initial_attempts_used > 1` consumes the shared budget:
    /// if the initial-call retry path used 3 attempts to even get a
    /// stream, only `MAX_ATTEMPTS - 3 = 2` mid-stream retries remain.
    /// Specifically, a stream-mid timeout with `initial_attempts_used = 4`
    /// must retry exactly once (one slot left) and then fatal.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_retry_shares_budget_with_initial_call() {
        tokio::time::pause();
        let initial = scripted_stream(vec![Err(timeout_err())]);
        // Two reconnect attempts available, but the wrapper should only
        // call reconnect once (4 already used + 1 retry = MAX_ATTEMPTS).
        let attempts_seen: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let attempts_seen_for = attempts_seen.clone();
        let reconnect = move || -> ReconnectFut {
            let attempts_seen = attempts_seen_for.clone();
            Box::pin(async move {
                *attempts_seen.lock().unwrap() += 1;
                Ok(scripted_stream(vec![Err(timeout_err())]))
            })
        };
        let (notify, captured) = send_notifier();

        let mut wrapped = stream_retry_wrap(
            initial,
            4, // initial-call retry already used 4 attempts
            "s-1".into(),
            Some("t-1".into()),
            1,
            notify,
            reconnect,
        );

        while let Some(item) = wrapped.next().await {
            // Consume; we only care about the structural assertions
            // below.
            let _ = item;
        }
        // Only one reconnect attempt — 4 (initial) + 1 (retry) = 5 = MAX.
        assert_eq!(*attempts_seen.lock().unwrap(), 1);
        // One retry event emitted.
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["attempt"], 5); // attempt-about-to-run is 5.
    }
}
