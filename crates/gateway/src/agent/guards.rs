//! Agent-loop safety nets — the M1 guard set.
//!
//! Defaults follow the locked design in docs/MILESTONES.md (Milestone 1) and
//! ARCHITECTURE.md §5. The stance (ARCHITECTURE §12.3): trust the provider;
//! observability is on by default, hard caps are opt-in.
//!
//! | guard                  | default        | rationale                                                            |
//! |------------------------|----------------|----------------------------------------------------------------------|
//! | `idle_timeout`         | 180 s, on      | M3.6: doubled from 90s — xhigh reasoning + 20+ tool turns regularly  |
//! |                        |                | go silent past 90s in the wild; T31b deep-dive prompts fataled.      |
//! | `wall_clock`           | 30 min, on     | claude-code removed a 5-min version as a bug; this is an edge ceiling |
//! | `max_iterations`       | 50, on         | M4.1.3 (P0-3): no longer opt-in. Main-agent failsafe — stress runs   |
//! |                        |                | hit 88+ tool calls before any natural stop; 50 lets normal deep work |
//! |                        |                | through but catches runaway loops. Subagents override per-AGENT.md.  |
//! | `cost_cap_usd`         | 5.0, on        | M4.1.3 (P0-4): no longer opt-in. Stress C1 ran 1500s with no cap;    |
//! |                        |                | $5/turn lets normal research through, catches the worst runaways.    |
//! | `doom_loop_threshold`  | 3, on          | leek-original; nobody else has it                                    |
//! | `auto_compact_threshold` | 0.90, on     | mirrors codex's `(context_window * 9) / 10`                          |
//! | `builtin_url_warn_threshold`  | 3, on   | warn at 3 repeats of the same `(action, url)` from codex builtin     |
//! | `builtin_url_abort_threshold` | 0, off  | M3.6: disabled by default — the main agent reading the same PDF 7    |
//! |                        |                | times under a long deep-dive is legitimate; abort killed real work.   |
//!
//! Resolution order (M2.6): `env var > config file > built-in default`. The
//! env var stays first so existing CI keeps working and an operator can
//! override one knob for a single run; the config file (`~/.leek/config.json`,
//! see `crate::config`) is the persistent user surface; the built-in default
//! is the fallback. `LEEK_IDLE_TIMEOUT_SECS` / `LEEK_WALL_CLOCK_SECS` (and
//! their config-file twins `idle_timeout_secs` / `wall_clock_secs`) accept
//! `0` to disable.

use std::collections::VecDeque;
use std::time::Duration;

use crate::config::Config;

// M3.7 dropped `Copy` — the new `reasoning_effort: String` field carries
// heap data. All call sites used `GuardConfig` by value or `&GuardConfig`
// already; the only behavioral change is that explicit cloning now goes
// through `.clone()` instead of the implicit copy.
#[derive(Debug, Clone)]
pub struct GuardConfig {
    pub idle_timeout: Option<Duration>,
    pub wall_clock: Option<Duration>,
    pub max_iterations: Option<usize>,
    pub cost_cap_usd: Option<f64>,
    pub doom_loop_threshold: Option<usize>,
    pub auto_compact_threshold: f32,
    /// Override for the model context window the auto-compaction trigger is
    /// sized against, in tokens. `None` → use the per-model `pricing` table.
    /// Set via `LEEK_CONTEXT_WINDOW` (or the `context_window` config-file
    /// field), mainly so a test can force a small window and trip compaction
    /// within a few turns.
    pub context_window: Option<i64>,
    /// M3.1: codex-builtin web_search duplicate-URL warn threshold (absolute
    /// count). Triggers a `codex_duplicate_url_warning` canvas event + a
    /// next-iter hint inject. `0` disables warning. Default 3. Env override:
    /// `LEEK_BUILTIN_URL_WARN_THRESHOLD`.
    pub builtin_url_warn_threshold: u32,
    /// M3.1: codex-builtin web_search duplicate-URL abort threshold
    /// (absolute count). When the same `(action_type, url)` is observed
    /// this many times the loop aborts the current iter with
    /// `stop_reason = "codex_duplicate_abort"`. `0` disables abort (the
    /// loop will keep warning indefinitely). **M3.6 default: 0 (off)** —
    /// the prior 7-repeat ceiling killed long deep-dive prompts that
    /// legitimately re-opened the same authoritative source under
    /// different angles. Env override: `LEEK_BUILTIN_URL_ABORT_THRESHOLD`.
    pub builtin_url_abort_threshold: u32,
    /// M3.7: main-agent reasoning effort passed into every codex
    /// `ChatRequest.reasoning_effort` field. One of
    /// `minimal`/`low`/`medium`/`high`/`xhigh`. Default `medium`
    /// (see `agent::REASONING_EFFORT_DEFAULT`). Env override:
    /// `LEEK_REASONING_EFFORT` — invalid values fall through to the
    /// config layer, then the built-in default. Subagents can override
    /// per-loop via `apply_agent_overrides` from AGENT.md frontmatter.
    pub reasoning_effort: String,
    /// M4.1.5: hard cap on tool calls per turn. The loop increments a
    /// turn-local `tool_call_count` for every dispatched tool call;
    /// when the count crosses the cap, the loop sets
    /// `stop_reason = "tool_call_cap_exceeded"` and breaks. `None`
    /// disables the cap. Default 30 for main-agent (stress round 3 A4
    /// showed 14 tool calls without converging on a single 60-day
    /// events question — 30 gives a wide enough margin for normal deep
    /// work but catches an over-thinking turn). Subagents override via
    /// the AGENT.md `max_tool_calls` frontmatter — `apply_agent_overrides`
    /// swaps this for the per-worker value (quick-screen: 8, comparison:
    /// 25, deep-review: 40, corpus-expert: 12, general-purpose: 30).
    /// Env override: `LEEK_MAX_TOOL_CALLS_PER_TURN`.
    pub max_tool_calls: Option<usize>,
}

impl GuardConfig {
    /// Build the guard set from the three layers: built-in defaults, then a
    /// config-file overlay, then env vars on top. The agent calls this at
    /// the start of every turn so a PATCH to the settings API takes effect
    /// without a restart.
    pub fn resolve(config: &Config) -> Self {
        Self {
            idle_timeout: duration_layered(
                "LEEK_IDLE_TIMEOUT_SECS",
                config.idle_timeout_secs,
                // M3.6: 180s. xhigh reasoning + 20+ tool turns regularly
                // exceed 90s of silence between SSE chunks; the prior 90s
                // default kept fataling long deep-dive prompts (T31b PM
                // critical). 180s gives the model enough rope to finish a
                // long tool batch without giving up on a genuinely stuck
                // stream.
                180,
            ),
            wall_clock: duration_layered(
                "LEEK_WALL_CLOCK_SECS",
                config.wall_clock_secs,
                30 * 60,
            ),
            // M4.1.3 (P0-3): main-agent failsafe — was opt-in (None
            // default); now ships at 50 so a runaway loop has a hard
            // ceiling. Stress runs saw 88+ tool calls before any
            // natural stop; 50 lets normal deep work through but
            // catches the runaway shape. User can still set explicit
            // 0 (or omit) to disable, via the resolver layering below.
            max_iterations: opt_usize_layered(
                "LEEK_MAX_ITERATIONS",
                config.max_iterations,
                Some(50),
            ),
            // M4.1.3 (P0-4): main-agent failsafe — was opt-in (None
            // default); now ships at $5/turn so the worst runaway
            // (stress C1 ran 1500s with no cap) hits a hard ceiling.
            // Subagents have their own per-AGENT.md caps that override
            // this in `apply_agent_overrides`.
            cost_cap_usd: opt_f64_layered(
                "LEEK_COST_CAP_USD",
                config.cost_cap_usd,
                Some(5.0),
            ),
            doom_loop_threshold: doom_threshold_layered(config.doom_loop_threshold),
            auto_compact_threshold: f32_layered(
                "LEEK_AUTO_COMPACT_THRESHOLD",
                config.auto_compact_threshold,
                0.90,
            ),
            context_window: context_window_layered(config.context_window),
            builtin_url_warn_threshold: u32_layered(
                "LEEK_BUILTIN_URL_WARN_THRESHOLD",
                config.builtin_url_warn_threshold,
                3,
            ),
            builtin_url_abort_threshold: u32_layered(
                "LEEK_BUILTIN_URL_ABORT_THRESHOLD",
                config.builtin_url_abort_threshold,
                // M3.6: 0 (disabled). The prior 7-repeat abort killed
                // legitimate deep-dive turns where the agent re-opened the
                // same authoritative source under different angles. The
                // warn surface (default 3) stays on, so duplicates remain
                // visible without taking the user's answer hostage.
                0,
            ),
            reasoning_effort: reasoning_effort_layered(
                config.reasoning_effort.as_deref(),
                // M3.7: medium is the new built-in. xhigh stays available
                // (Settings dropdown / `LEEK_REASONING_EFFORT=xhigh`) but
                // codex stream stability under xhigh + heavy builtin
                // search is fragile enough that the safer default ships
                // off. The deep-review subagent still runs at xhigh on
                // its isolated context.
                super::REASONING_EFFORT_DEFAULT,
            ),
            // M4.1.5: main-agent failsafe — 30 tool calls per turn.
            // Stress round 3 A4 over-thought a single 60-day events
            // question into 14 tool calls without converging; 30 is
            // wide enough for normal deep work, narrow enough to catch
            // the over-thinking shape. Subagents override per-AGENT.md.
            // 0 in env/config disables the cap (mirrors max_iterations
            // / cost_cap "0 = 不限" idiom).
            max_tool_calls: opt_usize_layered(
                "LEEK_MAX_TOOL_CALLS_PER_TURN",
                config.max_tool_calls_per_turn,
                Some(30),
            ),
        }
    }

    /// Build the guard set with no config-file overlay — env vars and
    /// built-in defaults only. Kept for tests and any code path that
    /// genuinely has no `Config` handle.
    pub fn from_env() -> Self {
        Self::resolve(&Config::default())
    }
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

/// On/off duration guard. Layering: env > config > default. An env or
/// config value of `0` disables the guard; an invalid env value logs and
/// falls through to the config layer.
fn duration_layered(env_key: &str, config_secs: Option<u64>, default_secs: u64) -> Option<Duration> {
    if let Some(s) = u64_env(env_key) {
        return if s == 0 { None } else { Some(Duration::from_secs(s)) };
    }
    if let Some(s) = config_secs {
        return if s == 0 { None } else { Some(Duration::from_secs(s)) };
    }
    Some(Duration::from_secs(default_secs))
}

/// Iteration cap. Env > config > built-in. An explicit `0` in env or
/// config is the product-defined "off" sentinel (mirrors the cost-cap
/// idiom): it disables the guard even if the built-in default would
/// have set one. M4.1.3 (P0-3): `built_in_default = Some(50)` for the
/// main-agent resolver makes 50 the failsafe; pass `None` for legacy
/// "opt-in" semantics.
fn opt_usize_layered(
    env_key: &str,
    config_value: Option<usize>,
    built_in_default: Option<usize>,
) -> Option<usize> {
    if let Some(v) = usize_env(env_key) {
        return if v > 0 { Some(v) } else { None };
    }
    if let Some(v) = config_value {
        return if v > 0 { Some(v) } else { None };
    }
    built_in_default.filter(|&n| n > 0)
}

/// USD cost cap. Env > config > built-in. Both `None` and explicit
/// `Some(0.0)` mean "guard off" at the env/config layer (product
/// decision 2026-05-22: 0 = 不限). M4.1.3 (P0-4): `built_in_default =
/// Some(5.0)` for the main-agent resolver makes $5 the failsafe; pass
/// `None` for legacy "opt-in" semantics.
fn opt_f64_layered(
    env_key: &str,
    config_value: Option<f64>,
    built_in_default: Option<f64>,
) -> Option<f64> {
    if let Some(v) = f64_env(env_key) {
        return if v > 0.0 && v.is_finite() { Some(v) } else { None };
    }
    if let Some(v) = config_value {
        return if v > 0.0 && v.is_finite() {
            Some(v)
        } else {
            // User picked 0 (or NaN) to explicitly disable — built-in
            // default does not get a second chance.
            None
        };
    }
    built_in_default.filter(|v| *v > 0.0 && v.is_finite())
}

/// Doom-loop threshold: env > config > default 3, must be ≥ 2.
fn doom_threshold_layered(config_value: Option<usize>) -> Option<usize> {
    if let Some(v) = usize_env("LEEK_DOOM_LOOP_THRESHOLD") {
        return if v >= 2 {
            Some(v)
        } else {
            tracing::warn!(value = v, "LEEK_DOOM_LOOP_THRESHOLD must be ≥ 2; using 3");
            Some(3)
        };
    }
    if let Some(v) = config_value {
        if v >= 2 {
            return Some(v);
        }
        tracing::warn!(value = v, "doom_loop_threshold in config must be ≥ 2; using 3");
    }
    Some(3)
}

/// Auto-compaction trigger fraction: env > config > default, must be in
/// `(0.0, 1.0]`.
fn f32_layered(env_key: &str, config_value: Option<f32>, default: f32) -> f32 {
    if let Some(v) = f32_env(env_key) {
        if v > 0.0 && v <= 1.0 {
            return v;
        }
        tracing::warn!(key = env_key, value = v, "invalid fraction env var; using config/default");
    }
    if let Some(v) = config_value {
        if v.is_finite() && v > 0.0 && v <= 1.0 {
            return v;
        }
        tracing::warn!(value = v, "invalid auto_compact_threshold in config; using default");
    }
    default
}

/// Optional context-window override: env > config > None (use per-model
/// table). `0` in either layer means "no override".
fn context_window_layered(config_value: Option<i64>) -> Option<i64> {
    if let Some(n) = usize_env("LEEK_CONTEXT_WINDOW") {
        return if n > 0 { Some(n as i64) } else { None };
    }
    config_value.filter(|&n| n > 0)
}

/// M3.7: layered string knob for `reasoning_effort`. Env > config >
/// default; invalid values at either layer fall through (with a warn)
/// so one typo never reaches the codex `ChatRequest`. Whitelist comes
/// from `config::is_valid_reasoning_effort` so the rule has a single
/// source of truth between the settings API and the resolver.
fn reasoning_effort_layered(config_value: Option<&str>, default: &str) -> String {
    if let Ok(raw) = std::env::var("LEEK_REASONING_EFFORT") {
        let trimmed = raw.trim();
        if crate::config::is_valid_reasoning_effort(trimmed) {
            return trimmed.to_string();
        }
        tracing::warn!(
            value = trimmed,
            "LEEK_REASONING_EFFORT must be one of minimal/low/medium/high/xhigh; falling through"
        );
    }
    if let Some(v) = config_value {
        let trimmed = v.trim();
        if crate::config::is_valid_reasoning_effort(trimmed) {
            return trimmed.to_string();
        }
        tracing::warn!(
            value = trimmed,
            "reasoning_effort in config is not a valid value; using default"
        );
    }
    default.to_string()
}

/// `u32` knob with env > config > default. `0` in env / config is preserved
/// as-is (it's a meaningful "disable" sentinel for the M3.1 builtin
/// duplicate-URL thresholds).
fn u32_layered(env_key: &str, config_value: Option<u32>, default: u32) -> u32 {
    if let Some(n) = u32_env(env_key) {
        return n;
    }
    config_value.unwrap_or(default)
}

fn u32_env(key: &str) -> Option<u32> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<u32>() {
        Ok(n) => Some(n),
        Err(_) => {
            tracing::warn!(key, raw = %v, "invalid u32 env var; falling through");
            None
        }
    }
}

fn u64_env(key: &str) -> Option<u64> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<u64>() {
        Ok(n) => Some(n),
        Err(_) => {
            tracing::warn!(key, raw = %v, "invalid u64 env var; falling through");
            None
        }
    }
}

fn usize_env(key: &str) -> Option<usize> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<usize>() {
        Ok(n) => Some(n),
        Err(_) => {
            tracing::warn!(key, raw = %v, "invalid usize env var; falling through");
            None
        }
    }
}

fn f64_env(key: &str) -> Option<f64> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<f64>() {
        Ok(n) => Some(n),
        Err(_) => {
            tracing::warn!(key, raw = %v, "invalid f64 env var; falling through");
            None
        }
    }
}

fn f32_env(key: &str) -> Option<f32> {
    let v = std::env::var(key).ok()?;
    match v.trim().parse::<f32>() {
        Ok(n) => Some(n),
        Err(_) => {
            tracing::warn!(key, raw = %v, "invalid f32 env var; falling through");
            None
        }
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
        // The resolver layers env over config; this test exercises the
        // env-only shortcut (`from_env`). It shares `ENV_LOCK` with the
        // M2.6 layering tests below — otherwise a parallel `set_var` from
        // another test would race the assertion.
        let _l = ENV_LOCK.lock().unwrap();
        std::env::remove_var("LEEK_CONTEXT_WINDOW");
        assert_eq!(GuardConfig::from_env().context_window, None);

        std::env::set_var("LEEK_CONTEXT_WINDOW", "16000");
        assert_eq!(GuardConfig::from_env().context_window, Some(16_000));

        // Invalid values fall through; here there's no config layer either.
        std::env::set_var("LEEK_CONTEXT_WINDOW", "not-a-number");
        assert_eq!(GuardConfig::from_env().context_window, None);

        std::env::remove_var("LEEK_CONTEXT_WINDOW");
    }

    // ── M2.6: env > config > default layering ─────────────────────────────
    //
    // These tests touch process-global env state, so they live behind a
    // mutex — `cargo test` runs them on different threads in one process,
    // and a parallel `set_var` from a different test would tear them.

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Scoped env var guard: restore the previous value on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn resolve_uses_built_in_default_when_neither_set() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::unset("LEEK_IDLE_TIMEOUT_SECS");
        let _g2 = EnvGuard::unset("LEEK_COST_CAP_USD");
        let _g3 = EnvGuard::unset("LEEK_AUTO_COMPACT_THRESHOLD");
        let _g4 = EnvGuard::unset("LEEK_MAX_ITERATIONS");

        let cfg = Config::default();
        let g = GuardConfig::resolve(&cfg);
        // M3.6: 180s built-in default (doubled from 90s for xhigh
        // reasoning + long-tool-batch turns; see resolve() comment).
        assert_eq!(g.idle_timeout, Some(Duration::from_secs(180)));
        // M4.1.3 (P0-4): cost_cap default = $5/turn, no longer None.
        assert_eq!(g.cost_cap_usd, Some(5.0));
        // M4.1.3 (P0-3): max_iterations default = 50, no longer None.
        assert_eq!(g.max_iterations, Some(50));
        assert!((g.auto_compact_threshold - 0.90).abs() < 1e-6);
    }

    #[test]
    fn resolve_uses_config_when_env_unset() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::unset("LEEK_IDLE_TIMEOUT_SECS");
        let _g2 = EnvGuard::unset("LEEK_COST_CAP_USD");
        let _g3 = EnvGuard::unset("LEEK_AUTO_COMPACT_THRESHOLD");

        let cfg = Config {
            idle_timeout_secs: Some(45),
            cost_cap_usd: Some(1.50),
            auto_compact_threshold: Some(0.75),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.idle_timeout, Some(Duration::from_secs(45)));
        assert_eq!(g.cost_cap_usd, Some(1.50));
        assert!((g.auto_compact_threshold - 0.75).abs() < 1e-6);
    }

    #[test]
    fn resolve_lets_env_override_config() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::set("LEEK_IDLE_TIMEOUT_SECS", "30");
        let _g2 = EnvGuard::set("LEEK_COST_CAP_USD", "2.5");

        let cfg = Config {
            idle_timeout_secs: Some(45),
            cost_cap_usd: Some(1.50),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        // env wins over config in both directions.
        assert_eq!(g.idle_timeout, Some(Duration::from_secs(30)));
        assert_eq!(g.cost_cap_usd, Some(2.5));
    }

    #[test]
    fn resolve_treats_zero_cost_cap_as_disabled() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("LEEK_COST_CAP_USD");

        let cfg = Config { cost_cap_usd: Some(0.0), ..Config::default() };
        // 0 is the product-defined "off" value (see config.rs). The
        // M4.1.3 built-in default ($5) does NOT get a second chance —
        // explicit 0 from the user means "no cap, period".
        assert_eq!(GuardConfig::resolve(&cfg).cost_cap_usd, None);
    }

    #[test]
    fn resolve_treats_zero_max_iterations_as_disabled() {
        // M4.1.3 (P0-3) symmetry: explicit 0 from env or config means
        // "no iteration cap" — overriding the new built-in 50 default.
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("LEEK_MAX_ITERATIONS");

        let cfg = Config { max_iterations: None, ..Config::default() };
        // Sanity: with default cfg, the failsafe applies.
        assert_eq!(GuardConfig::resolve(&cfg).max_iterations, Some(50));

        // 0 via env disables the cap.
        let _g_env = EnvGuard::set("LEEK_MAX_ITERATIONS", "0");
        assert_eq!(GuardConfig::resolve(&cfg).max_iterations, None);
    }

    #[test]
    fn resolve_builtin_url_thresholds_use_defaults() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::unset("LEEK_BUILTIN_URL_WARN_THRESHOLD");
        let _g2 = EnvGuard::unset("LEEK_BUILTIN_URL_ABORT_THRESHOLD");

        let g = GuardConfig::resolve(&Config::default());
        assert_eq!(g.builtin_url_warn_threshold, 3);
        // M3.6: abort disabled by default (was 7) — warn still on so
        // duplicates are visible; abort no longer kills deep-dive turns.
        assert_eq!(g.builtin_url_abort_threshold, 0);
    }

    #[test]
    fn resolve_builtin_url_thresholds_env_overrides_config() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::set("LEEK_BUILTIN_URL_WARN_THRESHOLD", "2");
        let _g2 = EnvGuard::set("LEEK_BUILTIN_URL_ABORT_THRESHOLD", "3");

        let cfg = Config {
            builtin_url_warn_threshold: Some(10),
            builtin_url_abort_threshold: Some(20),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.builtin_url_warn_threshold, 2);
        assert_eq!(g.builtin_url_abort_threshold, 3);
    }

    #[test]
    fn resolve_builtin_url_zero_thresholds_are_preserved() {
        // 0 在配置 / env 都保留 — tracker 用 0 当 "disable" 哨兵.
        let _l = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::unset("LEEK_BUILTIN_URL_WARN_THRESHOLD");
        let _g2 = EnvGuard::unset("LEEK_BUILTIN_URL_ABORT_THRESHOLD");

        let cfg = Config {
            builtin_url_warn_threshold: Some(0),
            builtin_url_abort_threshold: Some(0),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.builtin_url_warn_threshold, 0);
        assert_eq!(g.builtin_url_abort_threshold, 0);
    }

    // ── M3.7: reasoning_effort layering ───────────────────────────────

    #[test]
    fn resolve_uses_medium_reasoning_effort_default() {
        // The headline M3.7 change — fresh-default ships at `medium`,
        // not `xhigh`. A user upgrade onto an existing config file with
        // no `reasoning_effort` field gets the new default automatically.
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("LEEK_REASONING_EFFORT");
        let g = GuardConfig::resolve(&Config::default());
        assert_eq!(g.reasoning_effort, "medium");
    }

    #[test]
    fn resolve_config_overrides_default_reasoning_effort() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("LEEK_REASONING_EFFORT");
        let cfg = Config {
            reasoning_effort: Some("xhigh".into()),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.reasoning_effort, "xhigh");
    }

    #[test]
    fn resolve_env_overrides_config_reasoning_effort() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("LEEK_REASONING_EFFORT", "low");
        let cfg = Config {
            reasoning_effort: Some("xhigh".into()),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.reasoning_effort, "low");
    }

    #[test]
    fn resolve_falls_through_invalid_env_reasoning_effort() {
        // A typo in the env var (e.g. `LEEK_REASONING_EFFORT=hgh`) must
        // not poison the codex request — fall through to the config /
        // default rather than ship "hgh" as the effort.
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("LEEK_REASONING_EFFORT", "ultra");
        let cfg = Config {
            reasoning_effort: Some("high".into()),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        // Env value invalid → config layer wins.
        assert_eq!(g.reasoning_effort, "high");
    }

    #[test]
    fn resolve_falls_through_invalid_config_reasoning_effort() {
        // A bad value stored in the config (loaded from disk by a
        // hand-edited file that bypassed the API validator) must also
        // fall through rather than reach codex.
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("LEEK_REASONING_EFFORT");
        let cfg = Config {
            reasoning_effort: Some("super-mega".into()),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.reasoning_effort, "medium");
    }

    #[test]
    fn resolve_zero_seconds_disables_timer_guards() {
        let _l = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("LEEK_IDLE_TIMEOUT_SECS");
        let _g2 = EnvGuard::unset("LEEK_WALL_CLOCK_SECS");

        let cfg = Config {
            idle_timeout_secs: Some(0),
            wall_clock_secs: Some(0),
            ..Config::default()
        };
        let g = GuardConfig::resolve(&cfg);
        assert_eq!(g.idle_timeout, None);
        assert_eq!(g.wall_clock, None);
    }
}
