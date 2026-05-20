//! Agent-loop safety nets — the M1 guard set.
//!
//! Defaults follow the locked design in docs/MILESTONES.md (Milestone 1) and
//! ARCHITECTURE.md §5. The stance (ARCHITECTURE §12.3): trust the provider;
//! observability is on by default, hard caps are opt-in.
//!
//! | guard                  | default        | rationale                                                            |
//! |------------------------|----------------|----------------------------------------------------------------------|
//! | `idle_timeout`         | 90 s, on       | mirrors claude-code `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`             |
//! | `wall_clock`           | 30 min, on     | claude-code removed a 5-min version as a bug; this is an edge ceiling |
//! | `max_iterations`       | none, opt-in   | codex / claude-code do not enforce one                               |
//! | `cost_cap_usd`         | none, opt-in   | codex does not track cost                                            |
//! | `doom_loop_threshold`  | 3, on          | leek-original; nobody else has it                                    |
//! | `auto_compact_threshold` | 0.90, on     | mirrors codex's `(context_window * 9) / 10`                          |
//!
//! Until a settings UI exists, every guard is also overridable from the
//! environment — this is the only knob surface, and it makes the guards
//! testable. `LEEK_IDLE_TIMEOUT_SECS` / `LEEK_WALL_CLOCK_SECS` accept `0`
//! to disable.

use std::collections::VecDeque;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct GuardConfig {
    pub idle_timeout: Option<Duration>,
    pub wall_clock: Option<Duration>,
    pub max_iterations: Option<usize>,
    pub cost_cap_usd: Option<f64>,
    pub doom_loop_threshold: Option<usize>,
    pub auto_compact_threshold: f32,
    /// Override for the model context window the auto-compaction trigger is
    /// sized against, in tokens. `None` → use the per-model `pricing` table.
    /// Set via `LEEK_CONTEXT_WINDOW`, mainly so a test can force a small
    /// window and trip compaction within a few turns.
    pub context_window: Option<i64>,
}

impl GuardConfig {
    /// Build the guard set: locked defaults, each overridable by env var.
    pub fn from_env() -> Self {
        Self {
            idle_timeout: duration_env("LEEK_IDLE_TIMEOUT_SECS", 90),
            wall_clock: duration_env("LEEK_WALL_CLOCK_SECS", 30 * 60),
            max_iterations: opt_usize_env("LEEK_MAX_ITERATIONS"),
            cost_cap_usd: opt_f64_env("LEEK_COST_CAP_USD"),
            doom_loop_threshold: doom_threshold_env(),
            auto_compact_threshold: f32_env("LEEK_AUTO_COMPACT_THRESHOLD", 0.90),
            context_window: opt_usize_env("LEEK_CONTEXT_WINDOW").map(|n| n as i64),
        }
    }
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

/// On/off guard with a positive-seconds default; env `0` disables it.
fn duration_env(key: &str, default_secs: u64) -> Option<Duration> {
    match std::env::var(key) {
        Err(_) => Some(Duration::from_secs(default_secs)),
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(n) => Some(Duration::from_secs(n)),
            Err(_) => {
                tracing::warn!(key, raw = %v, "invalid duration env var; using default");
                Some(Duration::from_secs(default_secs))
            }
        },
    }
}

/// Opt-in cap: absent or invalid → `None` (guard off).
fn opt_usize_env(key: &str) -> Option<usize> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<usize>() {
        Ok(n) if n > 0 => Some(n),
        _ => {
            tracing::warn!(key, raw = %v, "invalid cap env var; guard left off");
            None
        }
    }
}

/// Opt-in USD cap: absent or invalid → `None` (guard off).
fn opt_f64_env(key: &str) -> Option<f64> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<f64>() {
        Ok(n) if n > 0.0 && n.is_finite() => Some(n),
        _ => {
            tracing::warn!(key, raw = %v, "invalid USD cap env var; guard left off");
            None
        }
    }
}

/// Doom-loop threshold: default 3, env override must be ≥ 2.
fn doom_threshold_env() -> Option<usize> {
    match std::env::var("LEEK_DOOM_LOOP_THRESHOLD") {
        Err(_) => Some(3),
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(n) if n >= 2 => Some(n),
            _ => {
                tracing::warn!(raw = %v, "invalid LEEK_DOOM_LOOP_THRESHOLD (need ≥ 2); using 3");
                Some(3)
            }
        },
    }
}

fn f32_env(key: &str, default: f32) -> f32 {
    match std::env::var(key) {
        Err(_) => default,
        Ok(v) => match v.trim().parse::<f32>() {
            Ok(n) if n > 0.0 && n <= 1.0 => n,
            _ => {
                tracing::warn!(key, raw = %v, "invalid fraction env var; using default");
                default
            }
        },
    }
}

/// Staged soft-prompt for the wall-clock guard. Given the seconds left in the
/// turn, returns an escalating nudge — or `None` when more than 10 minutes
/// remain, so a normal turn never sees the guard. Copy is locked in
/// docs/MILESTONES.md (decision 2026-05-09).
pub fn soft_deadline_hint(remaining_secs: u64) -> Option<&'static str> {
    match remaining_secs {
        0..=60 => Some("立刻收尾，用现有信息给结论，别再调工具。"),
        61..=120 => Some("现在写一个简洁的结论；完成已经在跑的工具调用，但不要开新的。"),
        121..=300 => Some("开始组织最终回答；非关键调查可以延后。"),
        301..=600 => Some("考虑缩小分析范围；如果还有多个分支，优先广度而非深度。"),
        _ => None,
    }
}

/// A doom loop is `threshold` identical `(tool, args)` calls in a row. The
/// caller keeps `window` trimmed to exactly `threshold`; if it drifted, the
/// detector refuses to fire (defends against stale state).
pub fn detect_doom_loop(window: &VecDeque<(String, String)>, threshold: usize) -> bool {
    if threshold < 2 || window.len() != threshold {
        return false;
    }
    let first = &window[0];
    window.iter().all(|c| c == first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(calls: &[(&str, &str)]) -> VecDeque<(String, String)> {
        calls
            .iter()
            .map(|(n, a)| (n.to_string(), a.to_string()))
            .collect()
    }

    #[test]
    fn soft_hint_is_none_above_ten_minutes() {
        assert!(soft_deadline_hint(601).is_none());
        assert!(soft_deadline_hint(10_000).is_none());
    }

    #[test]
    fn soft_hint_escalates() {
        assert!(soft_deadline_hint(600).unwrap().contains("缩小"));
        assert!(soft_deadline_hint(200).unwrap().contains("最终回答"));
        assert!(soft_deadline_hint(90).unwrap().contains("简洁"));
        assert!(soft_deadline_hint(10).unwrap().contains("立刻收尾"));
    }

    #[test]
    fn doom_loop_fires_on_n_identical() {
        let w = window(&[("web_fetch", "{}"), ("web_fetch", "{}"), ("web_fetch", "{}")]);
        assert!(detect_doom_loop(&w, 3));
    }

    #[test]
    fn doom_loop_ignores_below_threshold() {
        let w = window(&[("web_fetch", "{}"), ("web_fetch", "{}")]);
        assert!(!detect_doom_loop(&w, 3));
    }

    #[test]
    fn doom_loop_ignores_differing_args() {
        let w = window(&[
            ("web_fetch", "{\"t\":1}"),
            ("web_fetch", "{\"t\":2}"),
            ("web_fetch", "{\"t\":1}"),
        ]);
        assert!(!detect_doom_loop(&w, 3));
    }

    #[test]
    fn doom_loop_ignores_overflowed_window() {
        let w = window(&[("e", "{}"), ("e", "{}"), ("e", "{}"), ("e", "{}")]);
        assert!(!detect_doom_loop(&w, 3));
    }

    #[test]
    fn context_window_override_reads_env() {
        std::env::remove_var("LEEK_CONTEXT_WINDOW");
        assert_eq!(GuardConfig::from_env().context_window, None);

        std::env::set_var("LEEK_CONTEXT_WINDOW", "16000");
        assert_eq!(GuardConfig::from_env().context_window, Some(16_000));

        // Invalid values fall back to None, like the other opt-in env caps.
        std::env::set_var("LEEK_CONTEXT_WINDOW", "not-a-number");
        assert_eq!(GuardConfig::from_env().context_window, None);

        std::env::remove_var("LEEK_CONTEXT_WINDOW");
    }
}
