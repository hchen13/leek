//! M3.3: typed fatal-reason classification for a turn that died on the
//! codex side.
//!
//! Before M3.3 a fatal error became a single anyhow string — the user saw
//! "本回合调用失败：POST to the codex Responses endpoint" with no way to
//! tell network jitter from a 5xx from a malformed SSE. M3.3 splits that
//! into a small enum + a Chinese, actionable hint per kind, so the chat
//! card can tell the user whether to retry, check their token, or report
//! a bug.
//!
//! Classification path: `CodexClient` returns / surfaces typed
//! `CodexCallError`s (HTTP / Connect / Malformed). The agent loop catches
//! them via `anyhow::Error::downcast_ref` and calls
//! `FatalReason::from_call_error`. SSE-side errors (parse failures /
//! `provider returned error event` / etc.) bubble through the stream as
//! `Result<LlmEvent>` items; the loop runs them through the same
//! classifier (`from_stream_error`).
//!
//! Two new fields land on `turn_metrics` (migration 0006): `fatal_reason_kind`
//! (the discriminant string, e.g. `codex_http_5xx`) + `fatal_reason_detail`
//! (the human-readable body, e.g. `"codex backend returned 503"`). The
//! `assistant_done` event payload also carries a `fatal_reason` object so
//! the frontend can render a structured hint card without re-parsing
//! `fatal_error`.

use crate::llm::codex::CodexCallError;

/// Why a turn died fatally. One variant per *kind* of failure the user can
/// be told something useful about. The mapping is intentionally coarse — the
/// goal is a hint card that says either "retry", "fix your token", "check
/// the network", or "this is a leek bug; report it"; finer-grained reasons
/// would not change any of those four actions.
///
/// M3.4: retryable variants (5xx / connection_failed / stream_silent) carry
/// `retry_attempts`, the total number of times leek actually called codex
/// before giving up (1 = no retry was attempted, 3 = MAX_ATTEMPTS exhausted).
/// `hint()` weaves the count into its message so the user can see "已自动
/// 重试 3 次, 仍失败" instead of suspecting leek bailed on the first try.
/// Non-retryable variants (4xx / malformed) intentionally don't carry it —
/// they were never retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FatalReason {
    /// codex returned a 4xx — auth / quota / oversized request / etc. The
    /// body excerpt is what codex sent back (already redacted of Bearer
    /// tokens; truncated to ~200 chars).
    CodexHttp4xx { status: u16, body_excerpt: String },
    /// codex returned a 5xx — server-side trouble. Retry usually fixes it.
    /// `retry_attempts` counts initial call + retries that ran (M3.4); 1
    /// means leek tried once before giving up (no retry path), 3 means
    /// the full MAX_ATTEMPTS budget was exhausted.
    CodexHttp5xx {
        status: u16,
        body_excerpt: String,
        retry_attempts: u32,
    },
    /// The SSE stream went silent past the idle budget — the codex
    /// connection is up but no substantive event has arrived in `silent_secs`
    /// seconds. The user only sees this when the substantive-only idle
    /// detector trips (M3.3 §B). `retry_attempts` counts the silent-retry
    /// path (M3.4): 1 = no retry, 2 = the single silent-retry budget was
    /// used and still failed.
    CodexStreamSilent {
        silent_secs: u64,
        retry_attempts: u32,
    },
    /// Could not reach codex at all — DNS, connect refused, TLS handshake,
    /// connection reset before we got a response. The `detail` is the
    /// reqwest error chain (no secrets — codex's URL only). `retry_attempts`
    /// counts the same path as `CodexHttp5xx`.
    CodexConnectionFailed {
        detail: String,
        retry_attempts: u32,
    },
    /// codex replied with bytes that did not parse — broken SSE framing,
    /// unknown JSON envelope, an explicit `response.failed` event, etc. The
    /// user cannot do anything about this; the hint tells them it is a leek
    /// or codex bug worth reporting with the turn id.
    CodexMalformed { detail: String },
    /// Anything else — e.g. an internal storage error swallowed inside the
    /// loop. Kept as a fallback so an unclassified failure still gets a hint
    /// card instead of a raw stack-trace blob.
    Unknown { detail: String },
}

impl FatalReason {
    /// Stable, machine-readable kind string. Lives in `turn_metrics.fatal_reason_kind`
    /// and the `assistant_done` payload — the frontend renders off this.
    pub fn kind(&self) -> &'static str {
        match self {
            FatalReason::CodexHttp4xx { .. } => "codex_http_4xx",
            FatalReason::CodexHttp5xx { .. } => "codex_http_5xx",
            FatalReason::CodexStreamSilent { .. } => "codex_stream_silent",
            FatalReason::CodexConnectionFailed { .. } => "codex_connection_failed",
            FatalReason::CodexMalformed { .. } => "codex_malformed",
            FatalReason::Unknown { .. } => "unknown",
        }
    }

    /// Human-readable detail (Chinese, ~one sentence). Lives in
    /// `turn_metrics.fatal_reason_detail`. Includes a short, redacted body
    /// excerpt for HTTP errors so the user can paste it into a bug report
    /// without having to dig in the transcript archive.
    pub fn detail(&self) -> String {
        match self {
            FatalReason::CodexHttp4xx { status, body_excerpt } => {
                if body_excerpt.is_empty() {
                    format!("codex 返回 HTTP {status}")
                } else {
                    format!("codex 返回 HTTP {status}：{body_excerpt}")
                }
            }
            FatalReason::CodexHttp5xx {
                status,
                body_excerpt,
                ..
            } => {
                if body_excerpt.is_empty() {
                    format!("codex 返回 HTTP {status}")
                } else {
                    format!("codex 返回 HTTP {status}：{body_excerpt}")
                }
            }
            FatalReason::CodexStreamSilent { silent_secs, .. } => {
                format!("codex 连接静默 {silent_secs} 秒被超时杀")
            }
            FatalReason::CodexConnectionFailed { detail, .. } => {
                format!("无法连接 codex：{detail}")
            }
            FatalReason::CodexMalformed { detail } => {
                format!("codex 返回格式异常：{detail}")
            }
            FatalReason::Unknown { detail } => detail.clone(),
        }
    }

    /// Actionable hint shown in the chat card. Tells the user what to do
    /// (retry / check token / etc.) instead of just describing the failure.
    /// M3.4: retryable variants append "已自动重试 N 次" so the user can
    /// see leek didn't bail on the first failure — the retries actually
    /// happened and still failed.
    pub fn hint(&self) -> String {
        match self {
            FatalReason::CodexHttp4xx { .. } => {
                "请求被 codex 拒绝（可能 token 失效 / 请求超大），\
                 建议检查 Settings 里 token 或缩短 prompt 重试。"
                    .to_string()
            }
            FatalReason::CodexHttp5xx { retry_attempts, .. } => {
                let base = "codex 服务端临时不可用，稍后重试通常能解决。";
                with_retry_note(base, *retry_attempts)
            }
            FatalReason::CodexStreamSilent {
                silent_secs,
                retry_attempts,
            } => {
                let base = format!(
                    "codex 连接静默 {silent_secs} 秒被超时杀，\
                     可能网络抖动 / codex 临时过载，重试即可。"
                );
                with_retry_note(&base, *retry_attempts)
            }
            FatalReason::CodexConnectionFailed { retry_attempts, .. } => {
                let base = "无法连接 codex 服务（DNS / 网络），检查网络后重试。";
                with_retry_note(base, *retry_attempts)
            }
            FatalReason::CodexMalformed { .. } => {
                "codex 返回的格式 leek 没处理过，这是 leek 的 bug，\
                 请把这个 turn id 告诉开发者。"
                    .to_string()
            }
            FatalReason::Unknown { .. } => {
                "本回合因未知原因失败，请把 turn id 告诉开发者。".to_string()
            }
        }
    }

    /// JSON payload shipped on `assistant_done` for the frontend hint card.
    /// Four fields — `kind`, `detail`, `hint`, `retry_attempts` — keep the
    /// frontend a dumb renderer; the backend owns the i18n. The
    /// `retry_attempts` field is the same number `hint()` already weaves
    /// into the message; it's surfaced separately so a future frontend can
    /// render it as a distinct badge (e.g. a "retried 3×" pill) without
    /// re-parsing the Chinese hint string.
    pub fn to_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind(),
            "detail": self.detail(),
            "hint": self.hint(),
            "retry_attempts": self.retry_attempts(),
        })
    }

    /// Number of times leek actually called codex before giving up. `1`
    /// means the failure happened on the first attempt and no retry ran
    /// (non-retryable variant, or retry didn't apply — e.g. 4xx). For
    /// retryable variants this is the `retry_attempts` field; for the
    /// rest it's a fixed `1`.
    pub fn retry_attempts(&self) -> u32 {
        match self {
            FatalReason::CodexHttp4xx { .. }
            | FatalReason::CodexMalformed { .. }
            | FatalReason::Unknown { .. } => 1,
            FatalReason::CodexHttp5xx { retry_attempts, .. }
            | FatalReason::CodexStreamSilent { retry_attempts, .. }
            | FatalReason::CodexConnectionFailed { retry_attempts, .. } => *retry_attempts,
        }
    }

    /// Classify a typed `CodexCallError` (the failure mode for the initial
    /// HTTP POST / connect, before any SSE bytes have flowed). The
    /// `detail` strings are already redacted by the codex client.
    /// `retry_attempts = 1` for the retryable variants — `from_call_error`
    /// is the "first failure" entry point; the retry wrapper bumps the
    /// count in place via [`with_retry_attempts`] before surfacing the
    /// final reason.
    pub fn from_call_error(err: &CodexCallError) -> FatalReason {
        match err {
            CodexCallError::Http { status, body_excerpt } => {
                if (400..500).contains(status) {
                    FatalReason::CodexHttp4xx {
                        status: *status,
                        body_excerpt: body_excerpt.clone(),
                    }
                } else {
                    FatalReason::CodexHttp5xx {
                        status: *status,
                        body_excerpt: body_excerpt.clone(),
                        retry_attempts: 1,
                    }
                }
            }
            CodexCallError::Connection { detail } => FatalReason::CodexConnectionFailed {
                detail: detail.clone(),
                retry_attempts: 1,
            },
        }
    }

    /// Set `retry_attempts` on the retryable variants. No-op on the
    /// non-retryable ones (a 4xx that somehow flows through the retry
    /// layer keeps its `1`). Used by the retry wrapper to stamp the final
    /// FatalReason with the exact number of attempts it made.
    pub fn with_retry_attempts(mut self, attempts: u32) -> Self {
        match &mut self {
            FatalReason::CodexHttp5xx { retry_attempts, .. }
            | FatalReason::CodexStreamSilent { retry_attempts, .. }
            | FatalReason::CodexConnectionFailed { retry_attempts, .. } => {
                *retry_attempts = attempts;
            }
            _ => {}
        }
        self
    }

    /// Classify an error raised inside the SSE stream — either the SSE
    /// parser bailed (`provider returned an error event`, non-UTF-8, JSON
    /// parse failure) or the underlying byte stream returned a transport
    /// error. The current SSE parser surfaces all of these as `anyhow::Error`
    /// strings; we read the text to tell the two apart.
    ///
    /// Heuristic, not perfect — the SSE parser does not yet have a typed
    /// error enum. If it grows one, this becomes a `downcast_ref` like
    /// `from_call_error`. The downside of a missed classification is a
    /// "malformed" hint instead of a "connection_failed" one; both ask the
    /// user to retry / check the network, so the floor is acceptable.
    pub fn from_stream_error(err: &anyhow::Error) -> FatalReason {
        let msg = err.to_string();
        // The byte-stream side wraps its source as `reading SSE chunk`
        // (see `parse_sse_stream` in responses.rs). Treat that as a
        // connection problem — the bytes failed to flow.
        if msg.starts_with("reading SSE chunk") {
            return FatalReason::CodexConnectionFailed {
                detail: trimmed_error_chain(err),
                // First-failure default — the retry wrapper bumps this
                // via `with_retry_attempts` if it actually retried.
                retry_attempts: 1,
            };
        }
        FatalReason::CodexMalformed {
            detail: trimmed_error_chain(err),
        }
    }

    /// Fallback for an anyhow error we could not type — keep the chain
    /// available but tag it `unknown` so the frontend still renders a card.
    pub fn unclassified(err: &anyhow::Error) -> FatalReason {
        FatalReason::Unknown {
            detail: trimmed_error_chain(err),
        }
    }
}

/// M3.4: append a "已自动重试 N 次, 仍失败" note to the base hint when at
/// least one retry actually ran (`attempts >= 2` — the initial call
/// counts as attempt 1). Skipped on `attempts <= 1` so a never-retried
/// failure doesn't read as misleading "retried 1 time".
fn with_retry_note(base: &str, attempts: u32) -> String {
    if attempts <= 1 {
        return base.to_string();
    }
    let retries = attempts - 1;
    format!("{base}\n（已自动重试 {retries} 次，仍失败）")
}

/// Render an anyhow error + its cause chain into a single short string,
/// capped at ~300 chars so a runaway nested error does not blow up the
/// `turn_metrics` row size.
fn trimmed_error_chain(err: &anyhow::Error) -> String {
    let mut s = err.to_string();
    for cause in err.chain().skip(1) {
        s.push_str(" → ");
        s.push_str(&cause.to_string());
    }
    truncate_chars(&s, 300)
}

/// UTF-8-safe char truncation. anyhow chains often carry foreign error
/// strings — slicing by bytes can blow up at a multibyte boundary.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn http_4xx_is_classified_as_4xx() {
        let err = CodexCallError::Http {
            status: 401,
            body_excerpt: "{\"error\":\"token expired\"}".into(),
        };
        let r = FatalReason::from_call_error(&err);
        assert_eq!(r.kind(), "codex_http_4xx");
        assert!(r.detail().contains("401"));
        assert!(r.detail().contains("token expired"));
        assert!(r.hint().contains("token"));
    }

    #[test]
    fn http_5xx_is_classified_as_5xx() {
        let err = CodexCallError::Http {
            status: 503,
            body_excerpt: "service unavailable".into(),
        };
        let r = FatalReason::from_call_error(&err);
        assert_eq!(r.kind(), "codex_http_5xx");
        assert!(r.detail().contains("503"));
        assert!(r.hint().contains("稍后重试"));
    }

    #[test]
    fn connection_failure_is_classified_connection() {
        let err = CodexCallError::Connection {
            detail: "dns lookup failed".into(),
        };
        let r = FatalReason::from_call_error(&err);
        assert_eq!(r.kind(), "codex_connection_failed");
        assert!(r.detail().contains("dns"));
        assert!(r.hint().contains("网络"));
    }

    #[test]
    fn silent_carries_seconds_through_to_hint() {
        let r = FatalReason::CodexStreamSilent {
            silent_secs: 90,
            retry_attempts: 1,
        };
        assert_eq!(r.kind(), "codex_stream_silent");
        assert!(r.detail().contains("90"));
        assert!(r.hint().contains("90"));
    }

    #[test]
    fn malformed_keeps_chain_in_detail() {
        let inner = anyhow!("non-UTF-8 in SSE event");
        let outer = inner.context("parsing SSE data as JSON");
        let r = FatalReason::from_stream_error(&outer);
        assert_eq!(r.kind(), "codex_malformed");
        // chain is rendered with → separator
        assert!(r.detail().contains("→"));
    }

    #[test]
    fn reading_sse_chunk_routes_to_connection_failed() {
        let inner = anyhow!("error sending request");
        let outer = inner.context("reading SSE chunk");
        let r = FatalReason::from_stream_error(&outer);
        assert_eq!(r.kind(), "codex_connection_failed");
    }

    #[test]
    fn payload_carries_three_fields() {
        let r = FatalReason::CodexHttp5xx {
            status: 502,
            body_excerpt: "bad gateway".into(),
            retry_attempts: 1,
        };
        let p = r.to_payload();
        assert_eq!(p["kind"], "codex_http_5xx");
        assert!(p["detail"].as_str().unwrap().contains("502"));
        assert!(p["hint"].as_str().unwrap().contains("稍后重试"));
        // M3.4: payload also carries the retry_attempts field.
        assert_eq!(p["retry_attempts"], 1);
    }

    #[test]
    fn hint_appends_retry_count_when_retries_actually_ran() {
        // attempts = 1 → no retry note (the initial call counts as 1).
        let r1 = FatalReason::CodexHttp5xx {
            status: 503,
            body_excerpt: "".into(),
            retry_attempts: 1,
        };
        assert!(
            !r1.hint().contains("已自动重试"),
            "attempts=1 should not advertise retries"
        );
        // attempts = 3 → "已自动重试 2 次" (retries = attempts - 1).
        let r3 = FatalReason::CodexHttp5xx {
            status: 503,
            body_excerpt: "".into(),
            retry_attempts: 3,
        };
        let h = r3.hint();
        assert!(h.contains("已自动重试 2 次"), "hint was: {h}");
        assert!(h.contains("仍失败"));
    }

    #[test]
    fn with_retry_attempts_stamps_only_retryable_variants() {
        let mut r = FatalReason::CodexHttp5xx {
            status: 502,
            body_excerpt: "".into(),
            retry_attempts: 1,
        };
        r = r.with_retry_attempts(3);
        assert_eq!(r.retry_attempts(), 3);

        // Connection — also retryable, gets stamped.
        let r = FatalReason::CodexConnectionFailed {
            detail: "dns".into(),
            retry_attempts: 1,
        }
        .with_retry_attempts(2);
        assert_eq!(r.retry_attempts(), 2);

        // Silent — also retryable; stamped value sticks.
        let r = FatalReason::CodexStreamSilent {
            silent_secs: 60,
            retry_attempts: 1,
        }
        .with_retry_attempts(2);
        assert_eq!(r.retry_attempts(), 2);

        // 4xx — non-retryable; with_retry_attempts is a no-op on it.
        let r = FatalReason::CodexHttp4xx {
            status: 401,
            body_excerpt: "".into(),
        }
        .with_retry_attempts(3);
        assert_eq!(r.retry_attempts(), 1);

        // Malformed — non-retryable.
        let r = FatalReason::CodexMalformed {
            detail: "junk".into(),
        }
        .with_retry_attempts(3);
        assert_eq!(r.retry_attempts(), 1);
    }

    #[test]
    fn truncate_chars_handles_cjk_boundary() {
        let s = "中".repeat(400);
        let out = truncate_chars(&s, 300);
        assert_eq!(out.chars().count(), 301); // 300 + ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_chars_short_passthrough() {
        assert_eq!(truncate_chars("hello", 100), "hello");
    }

    #[test]
    fn unclassified_keeps_anyhow_chain() {
        let err = anyhow!("storage failed").context("inserting message");
        let r = FatalReason::unclassified(&err);
        assert_eq!(r.kind(), "unknown");
        assert!(r.detail().contains("→"));
    }

    /// M3.3 "smoke" — print the kind / detail / hint / payload for one
    /// instance of each FatalReason variant. Lets a reviewer run
    ///
    ///   cargo test --bin leek -p leek-gateway \
    ///       agent::fatal::tests::demo_each_kind_render -- --nocapture
    ///
    /// to eyeball the exact strings a user would see, without booting
    /// the gateway / faking a 5-minute silent codex stream.
    #[test]
    fn demo_each_kind_render() {
        let cases: Vec<(&str, FatalReason)> = vec![
            (
                "HTTP 5xx (codex backend down)",
                FatalReason::from_call_error(&CodexCallError::Http {
                    status: 503,
                    body_excerpt: "{\"error\":{\"message\":\"service temporarily unavailable\"}}"
                        .into(),
                }),
            ),
            (
                "HTTP 4xx (token expired)",
                FatalReason::from_call_error(&CodexCallError::Http {
                    status: 401,
                    body_excerpt: "{\"error\":\"access_token_expired\"}".into(),
                }),
            ),
            (
                "Connection failed (DNS / TCP / TLS)",
                FatalReason::from_call_error(&CodexCallError::Connection {
                    detail: "dns lookup failed: no such host".into(),
                }),
            ),
            (
                "Stream silent (90s idle budget tripped)",
                FatalReason::CodexStreamSilent {
                    silent_secs: 90,
                    retry_attempts: 2,
                },
            ),
            (
                "Malformed SSE (provider returned error event)",
                FatalReason::CodexMalformed {
                    detail: "parsing SSE data as JSON → non-UTF-8 in SSE event".into(),
                },
            ),
        ];

        for (label, r) in &cases {
            println!("\n--- {label} ---");
            println!("kind:    {}", r.kind());
            println!("detail:  {}", r.detail());
            println!("hint:    {}", r.hint());
            println!("payload: {}", r.to_payload());
        }
        // Sanity: every case produced a non-empty hint + kind — the
        // demo is only useful if the assertions pin the contract.
        for (_, r) in &cases {
            assert!(!r.kind().is_empty());
            assert!(!r.hint().is_empty());
            assert!(!r.detail().is_empty());
        }
    }
}
