//! Run hooks: turn a `(HookEvent, payload, target)` triple into a
//! `HookOutcome`. The engine spawns each matching command via `sh -c`,
//! feeds the JSON payload on stdin, captures stdout/stderr, and parses
//! the result per the exit-code semantics in research notes §5.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::config::{HookConfig, HookSet};
use super::outcome::{HookEvent, HookOutcome};

/// The runtime entry point used by the agent loop. Holds the merged
/// hook set; cheap to clone via `Arc` in `AppState`.
#[derive(Debug, Default)]
pub struct HookEngine {
    set: HookSet,
}

impl HookEngine {
    pub fn new(set: HookSet) -> Self {
        Self { set }
    }

    /// `true` if any hook is registered for this event. Used by the
    /// agent loop to skip payload building when nobody's listening.
    pub fn has_event(&self, event: HookEvent) -> bool {
        self.set.has_event(event)
    }

    /// Borrow the underlying `HookSet` — used by the read-only
    /// `GET /api/v1/hooks` endpoint.
    pub fn set(&self) -> &HookSet {
        &self.set
    }

    /// Trigger every hook registered for `event` whose matcher fires for
    /// `target`. Returns the first blocking outcome (with the chain
    /// stopped at that point — CC parity), or `Continue` if everyone
    /// approved / errored non-fatally.
    ///
    /// `target` is the value matched against the `matcher` field — for
    /// tool events it's the tool name; for other events it's empty (and
    /// matchers without a value match anything).
    ///
    /// Hooks are blocking — they run synchronously in the agent loop's
    /// thread. Each is bounded by its `timeout_secs`.
    pub async fn trigger(
        &self,
        event: HookEvent,
        target: &str,
        payload: serde_json::Value,
    ) -> HookOutcome {
        let matches = self.set.matching(event, target);
        if matches.is_empty() {
            return HookOutcome::Continue;
        }
        // Run hooks on a blocking-thread pool so a long-running shell
        // command doesn't stall the tokio reactor. Iterate serially —
        // ordering matters for "first block wins".
        for (_, hook) in matches {
            let payload_str = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
            let hook_clone = hook.clone();
            let event_name = event.as_str();
            let outcome = tokio::task::spawn_blocking(move || {
                run_one_hook(&hook_clone, &payload_str, event_name)
            })
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, event = event_name, "hook task panicked");
                HookOutcome::Continue
            });
            if outcome.is_block() && event.can_block() {
                return outcome;
            } else if outcome.is_block() {
                tracing::warn!(
                    event = event.as_str(),
                    "hook returned block but {} is not blockable — advisory only",
                    event.as_str()
                );
            }
        }
        HookOutcome::Continue
    }
}

/// Synchronous worker: spawn one hook, wait (with timeout), interpret
/// the result. Returns `Continue` for transport-level failures so a
/// user-side bug doesn't kill the agent loop.
fn run_one_hook(hook: &HookConfig, payload_json: &str, event_name: &str) -> HookOutcome {
    if hook.kind != "command" {
        tracing::warn!(
            kind = %hook.kind,
            event = event_name,
            "unsupported hook type — only 'command' is implemented this milestone"
        );
        return HookOutcome::Continue;
    }

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&hook.command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                event = event_name,
                command = %hook.command,
                error = %e,
                "failed to spawn hook"
            );
            return HookOutcome::Continue;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload_json.as_bytes());
        // stdin is dropped at scope end, signalling EOF to the child.
    }

    // Wait with a timeout — kill the child if it overruns.
    let timeout = Duration::from_secs(hook.timeout_secs.max(1));
    let start = Instant::now();
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::warn!(
                        event = event_name,
                        command = %hook.command,
                        timeout_secs = hook.timeout_secs,
                        "hook timed out — killed"
                    );
                    return HookOutcome::Continue;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                tracing::warn!(event = event_name, error = %e, "hook wait failed");
                return HookOutcome::Continue;
            }
        }
    };

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(event = event_name, error = %e, "hook wait_with_output failed");
            return HookOutcome::Continue;
        }
    };

    let exit_code = output.status.code();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    interpret_exit(exit_code, &stdout, &stderr, event_name)
}

/// Map a hook's (exit code, stdout, stderr) into a `HookOutcome`.
/// Public for unit tests.
pub(super) fn interpret_exit(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    event_name: &str,
) -> HookOutcome {
    match exit_code {
        Some(0) => parse_stdout_decision(stdout).unwrap_or(HookOutcome::Continue),
        Some(2) => {
            // CC's blocking error: feed stderr back to the model as the reason.
            let reason = stderr.trim();
            let reason = if reason.is_empty() {
                format!("hook for {event_name} exited 2 with no stderr")
            } else {
                reason.to_string()
            };
            HookOutcome::Block { reason }
        }
        Some(code) => {
            tracing::warn!(
                event = event_name,
                code,
                stderr = %stderr.trim(),
                "hook non-blocking error"
            );
            HookOutcome::Continue
        }
        None => {
            tracing::warn!(event = event_name, "hook terminated by signal");
            HookOutcome::Continue
        }
    }
}

/// Look for a CC-style JSON decision in the hook's stdout. Two shapes
/// are recognized (research notes §5):
///
/// 1. Top-level `{ "decision": "block", "reason": "..." }`
/// 2. `{ "hookSpecificOutput": { "permissionDecision": "deny", "permissionDecisionReason": "..." } }`
///
/// Anything else (non-JSON, or JSON without those fields) → Continue.
fn parse_stdout_decision(stdout: &str) -> Option<HookOutcome> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    // Top-level decision.
    if v.get("decision").and_then(|d| d.as_str()) == Some("block") {
        let reason = v
            .get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or("hook blocked the action")
            .to_string();
        return Some(HookOutcome::Block { reason });
    }
    // Pre-tool-use permission decision.
    if let Some(spec) = v.get("hookSpecificOutput") {
        if let Some(decision) = spec.get("permissionDecision").and_then(|d| d.as_str()) {
            if decision == "deny" {
                let reason = spec
                    .get("permissionDecisionReason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("hook denied permission")
                    .to_string();
                return Some(HookOutcome::Block { reason });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::config::HookSet;

    fn payload() -> serde_json::Value {
        serde_json::json!({ "tool_name": "web_fetch" })
    }

    fn set_with(raw: &str) -> HookSet {
        let file: serde_json::Value = serde_json::from_str(raw).unwrap();
        let mut set = HookSet::default();
        if let Some(map) = file
            .get("hooks")
            .and_then(|v| v.as_object())
        {
            for (event_name, entries) in map {
                let event = HookEvent::from_name(event_name).unwrap();
                let entries_typed: Vec<crate::hooks::config::HookMatcher> =
                    serde_json::from_value(entries.clone()).unwrap();
                set.by_event.insert(event, entries_typed);
            }
        }
        set
    }

    #[tokio::test]
    async fn no_hooks_registered_returns_continue() {
        let eng = HookEngine::new(HookSet::default());
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn successful_hook_yields_continue() {
        let raw = r#"{ "hooks": { "PreToolUse": [{ "matcher": "*", "hooks": [{ "command": "true" }] }] } }"#;
        let eng = HookEngine::new(set_with(raw));
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn exit_2_blocks_with_stderr_as_reason() {
        let raw = r#"
        {
          "hooks": {
            "PreToolUse": [{
              "matcher": "*",
              "hooks": [{ "command": "echo 'blocked: tool not allowed' >&2; exit 2" }]
            }]
          }
        }
        "#;
        let eng = HookEngine::new(set_with(raw));
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        match out {
            HookOutcome::Block { reason } => assert!(reason.contains("blocked: tool not allowed")),
            o => panic!("expected block, got {o:?}"),
        }
    }

    #[tokio::test]
    async fn stdout_json_decision_block_blocks() {
        let tmp = std::env::temp_dir().join(format!(
            "leek-hook-decision-block-{}.json",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::write(&tmp, r#"{"decision":"block","reason":"no"}"#).unwrap();
        let raw = format!(
            r#"{{ "hooks": {{ "PreToolUse": [{{ "matcher": "*", "hooks": [{{ "command": "cat {}" }}] }}] }} }}"#,
            tmp.display()
        );
        let eng = HookEngine::new(set_with(&raw));
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        let _ = std::fs::remove_file(&tmp);
        match out {
            HookOutcome::Block { reason } => assert_eq!(reason, "no"),
            o => panic!("expected block, got {o:?}"),
        }
    }

    #[tokio::test]
    async fn stdout_permission_deny_blocks_pre_tool_use() {
        // The hook command runs `cat <fixture file>` so the JSON payload
        // is delivered to stdout byte-for-byte (no fragile shell quoting).
        let tmp = std::env::temp_dir().join(format!(
            "leek-hook-permission-{}.json",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::write(
            &tmp,
            r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"policy"}}"#,
        )
        .unwrap();
        let raw = format!(
            r#"{{ "hooks": {{ "PreToolUse": [{{ "matcher": "*", "hooks": [{{ "command": "cat {}" }}] }}] }} }}"#,
            tmp.display()
        );
        let eng = HookEngine::new(set_with(&raw));
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        let _ = std::fs::remove_file(&tmp);
        match out {
            HookOutcome::Block { reason } => assert_eq!(reason, "policy"),
            o => panic!("expected block, got {o:?}"),
        }
    }

    #[tokio::test]
    async fn matcher_mismatch_skips_hook() {
        let raw = r#"
        {
          "hooks": {
            "PreToolUse": [{
              "matcher": "corpus_read",
              "hooks": [{ "command": "exit 2" }]
            }]
          }
        }
        "#;
        let eng = HookEngine::new(set_with(raw));
        // We trigger for `web_fetch`, but the only registered matcher is corpus_read.
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn first_block_short_circuits_remaining_hooks() {
        let raw = r#"
        {
          "hooks": {
            "PreToolUse": [{
              "matcher": "*",
              "hooks": [
                { "command": "exit 2" },
                { "command": "exit 99" }
              ]
            }]
          }
        }
        "#;
        let eng = HookEngine::new(set_with(raw));
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        // The second hook's exit 99 would also be observed by the engine
        // if iteration didn't stop at the first block.
        assert!(out.is_block());
    }

    #[tokio::test]
    async fn block_at_post_tool_use_is_advisory_only() {
        let raw = r#"
        {
          "hooks": {
            "PostToolUse": [{
              "matcher": "*",
              "hooks": [{ "command": "exit 2" }]
            }]
          }
        }
        "#;
        let eng = HookEngine::new(set_with(raw));
        let out = eng.trigger(HookEvent::PostToolUse, "web_fetch", payload()).await;
        // Post-events can't actually block — the outcome must be Continue
        // even though the hook tried to block.
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn timeout_does_not_block() {
        let raw = r#"
        {
          "hooks": {
            "PreToolUse": [{
              "matcher": "*",
              "hooks": [{ "command": "sleep 5", "timeout_secs": 1 }]
            }]
          }
        }
        "#;
        let eng = HookEngine::new(set_with(raw));
        let started = std::time::Instant::now();
        let out = eng.trigger(HookEvent::PreToolUse, "web_fetch", payload()).await;
        // Timeout is treated as non-blocking error.
        assert_eq!(out, HookOutcome::Continue);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[tokio::test]
    async fn payload_is_delivered_to_hook_on_stdin() {
        // End-to-end: an installed hook captures the JSON payload that
        // would be delivered by the agent loop and writes it out. This
        // mirrors what the manual `~/.leek/config.json` test would do.
        let log = std::env::temp_dir().join(format!(
            "leek-hook-payload-{}.json",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let raw = format!(
            r#"{{ "hooks": {{ "PreToolUse": [{{ "matcher": "*", "hooks": [{{ "command": "cat > {} ; exit 0" }}] }}] }} }}"#,
            log.display()
        );
        let eng = HookEngine::new(set_with(&raw));
        let payload = serde_json::json!({
            "session_id": "s-test",
            "turn_id": "t-test",
            "iteration": 1,
            "hook_event_name": "PreToolUse",
            "tool_name": "web_fetch",
            "tool_input": { "url": "https://example.com" },
            "tool_use_id": "call-xyz"
        });
        let out = eng
            .trigger(HookEvent::PreToolUse, "web_fetch", payload)
            .await;
        assert_eq!(out, HookOutcome::Continue);

        let logged = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        let parsed: serde_json::Value = serde_json::from_str(&logged).unwrap();
        assert_eq!(parsed["tool_name"], "web_fetch");
        assert_eq!(parsed["session_id"], "s-test");
        assert_eq!(parsed["hook_event_name"], "PreToolUse");
        assert_eq!(parsed["tool_input"]["url"], "https://example.com");
    }

    #[test]
    fn interpret_exit_handles_known_codes() {
        let block = interpret_exit(Some(2), "", "boom", "PreToolUse");
        assert_eq!(block, HookOutcome::Block { reason: "boom".into() });
        let cont = interpret_exit(Some(0), "", "", "PreToolUse");
        assert_eq!(cont, HookOutcome::Continue);
        let other = interpret_exit(Some(7), "", "non-fatal", "PreToolUse");
        assert_eq!(other, HookOutcome::Continue);
        let sig = interpret_exit(None, "", "", "PreToolUse");
        assert_eq!(sig, HookOutcome::Continue);
    }
}
