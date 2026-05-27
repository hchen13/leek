//! User-tunable settings — persisted as `~/.leek/config.json` (M2.6).
//!
//! This is the *user-facing* knob surface for the M1 guard set. The guards
//! themselves have always shipped with sane defaults and a hidden env-var
//! override (see `agent::guards`); M2.6 adds the JSON file plus the
//! `GET/PATCH /api/v1/settings` API so a user can persist their cost cap (or
//! any other guard) without editing shell rc files.
//!
//! ## Layering
//!
//! Resolution order, strictest first: `env var > config file > built-in
//! default`. That is, an env var still wins — both so existing CI keeps
//! working unchanged and so an operator can override one knob for a single
//! run without rewriting the file. `GuardConfig::resolve` in `agent::guards`
//! is the one place this layering happens.
//!
//! ## Shape
//!
//! Every field is `Option<T>` and the file is a partial document: a brand
//! new install has no file (the `load()` call returns `Config::default()`
//! and never errors), and a PATCH that sets one field leaves the others
//! `None`. The defaults live in `guards.rs`, not here — this file only
//! says "user has not picked a value yet".
//!
//! `cost_cap_usd = Some(0.0)` is the user-facing "0 = 不限制" idiom from the
//! 2026-05-22 product decision: 0 disables the cap, the same as the guard's
//! built-in absent value. `GuardConfig::resolve` treats both the same.
//!
//! Writes are atomic: a temp file in the same directory is `rename`d into
//! place, so a crash mid-write cannot leave a half-written `config.json`.
//!
//! A malformed file is non-fatal at startup — the gateway logs a warning
//! and treats the file as empty. The user can then PATCH the API to fix it
//! (and the next write rewrites the file cleanly). The alternative — abort
//! on parse error — would let one bad edit lock the user out of the API
//! they need to fix the file.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted user settings — see the module doc for layering rules.
///
/// `#[serde(default)]` on the struct means an empty `{}` document parses
/// cleanly into `Config::default()`; absent fields stay `None`. The same
/// flag on each field guards against a partial PATCH dropping the others
/// when we round-trip through `serde_json::from_value`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Stream-idle timeout in seconds. `0` disables the guard.
    pub idle_timeout_secs: Option<u64>,
    /// Per-turn wall-clock budget in seconds. `0` disables the guard.
    pub wall_clock_secs: Option<u64>,
    /// Cap on LLM iterations per turn. `None` = no cap.
    pub max_iterations: Option<usize>,
    /// USD cost cap per turn. `0.0` or `None` disables the cap (product
    /// decision 2026-05-22: 0 = 不限).
    pub cost_cap_usd: Option<f64>,
    /// Identical-`(tool, args)` calls in a row before the doom-loop guard
    /// trips. Must be ≥ 2.
    pub doom_loop_threshold: Option<usize>,
    /// Auto-compaction trigger as a fraction of the context window. Must
    /// be in `(0.0, 1.0]`.
    pub auto_compact_threshold: Option<f32>,
    /// Override for the model context window in tokens (mainly for tests
    /// that want to trip compaction without burning a real long turn).
    pub context_window: Option<i64>,
    /// M3: persisted Tushare token for A-share research tools. The env
    /// var `LEEK_TUSHARE_TOKEN` takes precedence at startup. Settings
    /// API never echoes this back masked-or-otherwise — it stays
    /// write-only from the API to avoid leaking via diagnostics.
    pub tushare_token: Option<String>,
    /// M3.1: per-turn warn threshold for the codex-builtin web_search
    /// duplicate-URL tracker. The agent emits a canvas warning + injects
    /// a hint into the next iter once the same `(action_type, url)` has
    /// been opened ≥ this many times. `0` disables warning. Default 3.
    pub builtin_url_warn_threshold: Option<u32>,
    /// M3.1: per-turn abort threshold. When the same `(action_type, url)`
    /// is opened ≥ this many times the agent aborts the current iter
    /// with `stop_reason = "codex_duplicate_abort"`. `0` disables abort
    /// (warn-only). Default 7.
    pub builtin_url_abort_threshold: Option<u32>,
    /// M3.7: main-agent reasoning effort. One of
    /// `minimal`/`low`/`medium`/`high`/`xhigh`. `None` falls back to the
    /// built-in default (medium — see `agent::REASONING_EFFORT_DEFAULT`).
    /// Subagent overrides come from the AGENT.md `reasoning_effort`
    /// frontmatter field (see `agents::AgentDef`); this field only
    /// affects the main agent's loop.
    pub reasoning_effort: Option<String>,
    /// M4.1.5: hard cap on tool calls per turn. `None` falls back to the
    /// built-in default (30 — see `agent::guards::resolve`). `0` disables
    /// the cap (mirrors the `max_iterations` idiom). Stress round 3 A4
    /// showed the main agent at medium effort over-thinking a single
    /// 60-day events question into 14 tool calls without converging;
    /// iter / cost / wall-clock caps did not catch it. Per-subagent
    /// overrides come from the AGENT.md `max_tool_calls` frontmatter
    /// field (see `agents::AgentDef`).
    pub max_tool_calls_per_turn: Option<usize>,
}

impl Config {
    /// Read the config file. A missing file returns `Config::default()`; a
    /// malformed file logs a warning and returns `Config::default()` — see
    /// the module doc for why we never fail at this layer.
    pub fn load() -> Self {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not read leek config; using defaults"
                );
                Self::default()
            }
            Ok(text) => match serde_json::from_str(&text) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "leek config is malformed JSON; using defaults"
                    );
                    Self::default()
                }
            },
        }
    }

    /// Atomically write the config to disk. Creates `~/.leek/` if needed.
    /// Writes go to a sibling `.tmp` file first, then `rename` — so a crash
    /// mid-write cannot leave the live file truncated.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let body = serde_json::to_vec_pretty(self).context("serializing config")?;
        let tmp = path.with_extension("json.tmp");
        // Drop the file handle (closing it) before the rename — Windows
        // would error otherwise. `flush + sync` give us durability so the
        // following rename can't see a half-written file.
        {
            let mut f = fs::File::create(&tmp)
                .with_context(|| format!("opening {} for write", tmp.display()))?;
            f.write_all(&body)
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.flush().ok();
            // Best-effort fsync — we still rename even if this fails.
            let _ = f.sync_all();
        }
        fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
        Ok(())
    }

    /// `~/.leek/config.json` — or `$LEEK_CONFIG_DIR/config.json` when that
    /// is set (the test suite uses it to keep `$HOME` untouched).
    pub fn path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    /// Resolution: `$LEEK_CONFIG_DIR` → `$HOME/.leek` → `./.leek` (last
    /// resort, so the binary still starts on an exotic env with no home).
    pub fn config_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("LEEK_CONFIG_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
        if let Some(home) = home_dir() {
            return home.join(".leek");
        }
        PathBuf::from(".leek")
    }

    /// Merge a partial document onto this one — every `Some` in `patch`
    /// overwrites the corresponding field, `None` leaves it alone. Kept
    /// for compatibility with internal call sites and the test suite; the
    /// public settings PATCH endpoint uses [`Config::merge_patch`] so
    /// `"field": null` clears the field instead of silently noop'ing.
    #[allow(dead_code)]
    pub fn merge(&mut self, patch: Config) {
        if patch.idle_timeout_secs.is_some() {
            self.idle_timeout_secs = patch.idle_timeout_secs;
        }
        if patch.wall_clock_secs.is_some() {
            self.wall_clock_secs = patch.wall_clock_secs;
        }
        if patch.max_iterations.is_some() {
            self.max_iterations = patch.max_iterations;
        }
        if patch.cost_cap_usd.is_some() {
            self.cost_cap_usd = patch.cost_cap_usd;
        }
        if patch.doom_loop_threshold.is_some() {
            self.doom_loop_threshold = patch.doom_loop_threshold;
        }
        if patch.auto_compact_threshold.is_some() {
            self.auto_compact_threshold = patch.auto_compact_threshold;
        }
        if patch.context_window.is_some() {
            self.context_window = patch.context_window;
        }
        if patch.tushare_token.is_some() {
            self.tushare_token = patch.tushare_token;
        }
        if patch.builtin_url_warn_threshold.is_some() {
            self.builtin_url_warn_threshold = patch.builtin_url_warn_threshold;
        }
        if patch.builtin_url_abort_threshold.is_some() {
            self.builtin_url_abort_threshold = patch.builtin_url_abort_threshold;
        }
        if patch.reasoning_effort.is_some() {
            self.reasoning_effort = patch.reasoning_effort;
        }
        if patch.max_tool_calls_per_turn.is_some() {
            self.max_tool_calls_per_turn = patch.max_tool_calls_per_turn;
        }
    }

    /// M3.6: PATCH-style merge that distinguishes **absent** field
    /// (JSON does not mention it → noop) from **explicit null** (JSON
    /// `"field": null` → clear the field, falling back to the resolver's
    /// built-in default). The body of `PATCH /api/v1/settings` deserializes
    /// into `ConfigPatch` via `double_option`, so the handler can call
    /// this without losing the distinction.
    ///
    /// Pre-M3.6 the settings API used [`Config::merge`], which treated
    /// `None` as "leave alone" and gave the user no way to clear a stored
    /// value: PATCH `{"cost_cap_usd": null}` was a silent no-op. The
    /// frontend's "Reset to default" button needs the clear semantics to
    /// roll back a fresh-install user's whole stored doc to defaults.
    pub fn merge_patch(&mut self, patch: ConfigPatch) {
        if let Some(v) = patch.idle_timeout_secs {
            self.idle_timeout_secs = v;
        }
        if let Some(v) = patch.wall_clock_secs {
            self.wall_clock_secs = v;
        }
        if let Some(v) = patch.max_iterations {
            self.max_iterations = v;
        }
        if let Some(v) = patch.cost_cap_usd {
            self.cost_cap_usd = v;
        }
        if let Some(v) = patch.doom_loop_threshold {
            self.doom_loop_threshold = v;
        }
        if let Some(v) = patch.auto_compact_threshold {
            self.auto_compact_threshold = v;
        }
        if let Some(v) = patch.context_window {
            self.context_window = v;
        }
        if let Some(v) = patch.tushare_token {
            self.tushare_token = v;
        }
        if let Some(v) = patch.builtin_url_warn_threshold {
            self.builtin_url_warn_threshold = v;
        }
        if let Some(v) = patch.builtin_url_abort_threshold {
            self.builtin_url_abort_threshold = v;
        }
        if let Some(v) = patch.reasoning_effort {
            self.reasoning_effort = v;
        }
        if let Some(v) = patch.max_tool_calls_per_turn {
            self.max_tool_calls_per_turn = v;
        }
    }

    /// Validate user-supplied values: numeric ranges only — we do not check
    /// "does this make sense for your machine", that is the user's call.
    /// Returns the offending fields and a human-readable reason each.
    pub fn validate(&self) -> Result<(), Vec<ConfigFieldError>> {
        let mut errs = Vec::new();
        if let Some(cap) = self.cost_cap_usd {
            if !cap.is_finite() || cap < 0.0 {
                errs.push(ConfigFieldError::new(
                    "cost_cap_usd",
                    "must be a non-negative number (0 disables the cap)",
                ));
            }
        }
        if let Some(n) = self.max_iterations {
            if n == 0 {
                errs.push(ConfigFieldError::new(
                    "max_iterations",
                    "must be ≥ 1 (omit the field to disable the cap)",
                ));
            }
        }
        if let Some(n) = self.doom_loop_threshold {
            if n < 2 {
                errs.push(ConfigFieldError::new(
                    "doom_loop_threshold",
                    "must be ≥ 2",
                ));
            }
        }
        if let Some(t) = self.auto_compact_threshold {
            if !t.is_finite() || t <= 0.0 || t > 1.0 {
                errs.push(ConfigFieldError::new(
                    "auto_compact_threshold",
                    "must be in (0.0, 1.0]",
                ));
            }
        }
        if let Some(n) = self.context_window {
            if n <= 0 {
                errs.push(ConfigFieldError::new(
                    "context_window",
                    "must be ≥ 1 (omit the field to use the model default)",
                ));
            }
        }
        if let Some(ref effort) = self.reasoning_effort {
            if !is_valid_reasoning_effort(effort) {
                errs.push(ConfigFieldError::new(
                    "reasoning_effort",
                    "must be one of: minimal, low, medium, high, xhigh",
                ));
            }
        }
        // M4.1.5: max_tool_calls_per_turn — `0` is the user-facing
        // "disable" idiom (mirrors max_iterations); negative is impossible
        // because the type is `usize`. Nothing to reject here beyond shape,
        // but the field still flows through the standard layered resolver.
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// M3.7: validate a `reasoning_effort` string against the 5-value
/// whitelist codex accepts. Shared between [`Config::validate`] and
/// [`ConfigPatch::validate`] so the rule has one source of truth.
pub(crate) fn is_valid_reasoning_effort(s: &str) -> bool {
    matches!(s, "minimal" | "low" | "medium" | "high" | "xhigh")
}

/// One field-level error from `Config::validate`. Surfaced to the user via
/// the settings API's 400 response.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigFieldError {
    pub field: &'static str,
    pub message: &'static str,
}

impl ConfigFieldError {
    fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

/// M3.6 PATCH-shape twin of [`Config`]. Every field is `Option<Option<T>>`
/// using `double_option` so the deserializer can tell apart three states:
///
/// | JSON                            | Rust                  | merge effect      |
/// |---------------------------------|-----------------------|-------------------|
/// | field absent                    | `None`                | leave alone       |
/// | `"field": null`                 | `Some(None)`          | **clear** field    |
/// | `"field": <value>`              | `Some(Some(v))`       | set to `v`        |
///
/// `deny_unknown_fields` mirrors `Config` so a misspelled field 400s
/// instead of silently going through.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigPatch {
    #[serde(deserialize_with = "double_option", default)]
    pub idle_timeout_secs: Option<Option<u64>>,
    #[serde(deserialize_with = "double_option", default)]
    pub wall_clock_secs: Option<Option<u64>>,
    #[serde(deserialize_with = "double_option", default)]
    pub max_iterations: Option<Option<usize>>,
    #[serde(deserialize_with = "double_option", default)]
    pub cost_cap_usd: Option<Option<f64>>,
    #[serde(deserialize_with = "double_option", default)]
    pub doom_loop_threshold: Option<Option<usize>>,
    #[serde(deserialize_with = "double_option", default)]
    pub auto_compact_threshold: Option<Option<f32>>,
    #[serde(deserialize_with = "double_option", default)]
    pub context_window: Option<Option<i64>>,
    #[serde(deserialize_with = "double_option", default)]
    pub tushare_token: Option<Option<String>>,
    #[serde(deserialize_with = "double_option", default)]
    pub builtin_url_warn_threshold: Option<Option<u32>>,
    #[serde(deserialize_with = "double_option", default)]
    pub builtin_url_abort_threshold: Option<Option<u32>>,
    #[serde(deserialize_with = "double_option", default)]
    pub reasoning_effort: Option<Option<String>>,
    #[serde(deserialize_with = "double_option", default)]
    pub max_tool_calls_per_turn: Option<Option<usize>>,
}

impl ConfigPatch {
    /// Same numeric-range checks as [`Config::validate`], but only for
    /// fields explicitly set to a value (`Some(Some(_))`). A field
    /// cleared by explicit null (`Some(None)`) skips validation — clearing
    /// a stored field cannot be "invalid". Absent fields obviously skip.
    pub fn validate(&self) -> Result<(), Vec<ConfigFieldError>> {
        let mut errs = Vec::new();
        if let Some(Some(cap)) = self.cost_cap_usd {
            if !cap.is_finite() || cap < 0.0 {
                errs.push(ConfigFieldError::new(
                    "cost_cap_usd",
                    "must be a non-negative number (0 disables the cap)",
                ));
            }
        }
        if let Some(Some(n)) = self.max_iterations {
            if n == 0 {
                errs.push(ConfigFieldError::new(
                    "max_iterations",
                    "must be ≥ 1 (omit the field to disable the cap)",
                ));
            }
        }
        if let Some(Some(n)) = self.doom_loop_threshold {
            if n < 2 {
                errs.push(ConfigFieldError::new(
                    "doom_loop_threshold",
                    "must be ≥ 2",
                ));
            }
        }
        if let Some(Some(t)) = self.auto_compact_threshold {
            if !t.is_finite() || t <= 0.0 || t > 1.0 {
                errs.push(ConfigFieldError::new(
                    "auto_compact_threshold",
                    "must be in (0.0, 1.0]",
                ));
            }
        }
        if let Some(Some(n)) = self.context_window {
            if n <= 0 {
                errs.push(ConfigFieldError::new(
                    "context_window",
                    "must be ≥ 1 (omit the field to use the model default)",
                ));
            }
        }
        if let Some(Some(ref effort)) = self.reasoning_effort {
            if !is_valid_reasoning_effort(effort) {
                errs.push(ConfigFieldError::new(
                    "reasoning_effort",
                    "must be one of: minimal, low, medium, high, xhigh",
                ));
            }
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// serde shim — make `T` deserialize into `Option<Option<T>>` such that
/// a missing key stays `None`, an explicit `null` becomes `Some(None)`,
/// and any other value becomes `Some(Some(v))`. This is the standard
/// "JSON Merge Patch" workaround for distinguishing absent / null /
/// present in serde-json (which otherwise collapses absent and null
/// into the same `None`).
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// `$HOME` resolution that works on every platform we ship for without
/// pulling in `dirs` — std doesn't expose a stable home_dir, but the env
/// vars below are conventional and available everywhere we run.
fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    // Windows fallback — we don't target Windows today but this keeps the
    // function honest if someone tries.
    if let Ok(h) = std::env::var("USERPROFILE") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize every test that touches `LEEK_CONFIG_DIR` (a process-global
    /// env var). Without this, two `scoped()` calls on different threads
    /// race: thread A sets the dir, thread B overwrites it, A's `cfg.save()`
    /// then writes to B's path and B's `cfg.load()` reads the wrong file.
    /// (Pre-M3.6 the suite was flaky for exactly this reason; M3.6 makes it
    /// deterministic by enforcing serial execution at the scoping helper.)
    static CONFIG_DIR_LOCK: Mutex<()> = Mutex::new(());

    /// Point `Config::path()` at a uuid-named scratch dir for the lifetime
    /// of one test. Returns the dir + a guard that restores the previous
    /// env value on drop, so parallel tests don't trample each other.
    struct ScopedConfigDir {
        _dir: tempdir::TempDir,
        prev: Option<String>,
        // The lock outlives the env-var manipulation: while we hold it,
        // no other config test can see (or overwrite) our LEEK_CONFIG_DIR.
        // `Option` so Drop can take ownership without `Default::default()`.
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for ScopedConfigDir {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("LEEK_CONFIG_DIR", v),
                None => std::env::remove_var("LEEK_CONFIG_DIR"),
            }
            // _lock drops here, releasing the mutex.
        }
    }

    /// Local-only thin shim around a uuid-named tempdir: we don't pull in
    /// the `tempdir` crate, this is a hand-rolled equivalent.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct TempDir {
            path: PathBuf,
        }
        impl TempDir {
            pub fn new(prefix: &str) -> std::io::Result<Self> {
                let base = std::env::temp_dir().join(format!(
                    "{prefix}-{}",
                    uuid::Uuid::new_v4().simple()
                ));
                std::fs::create_dir_all(&base)?;
                Ok(Self { path: base })
            }
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    fn scoped() -> ScopedConfigDir {
        // Acquire the shared lock FIRST; the env-var manipulation below
        // must happen with no other config test in flight. If a previous
        // test panicked the lock would be poisoned — recover by extracting
        // the guard, since the env var is the only shared state we care
        // about and we restore it on drop.
        let lock = CONFIG_DIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir::TempDir::new("leek-config-test").unwrap();
        let prev = std::env::var("LEEK_CONFIG_DIR").ok();
        std::env::set_var("LEEK_CONFIG_DIR", dir.path());
        ScopedConfigDir { _dir: dir, prev, _lock: lock }
    }

    #[test]
    fn load_with_no_file_returns_default() {
        let _g = scoped();
        let cfg = Config::load();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let _g = scoped();
        let mut cfg = Config::default();
        cfg.cost_cap_usd = Some(0.50);
        cfg.idle_timeout_secs = Some(45);
        cfg.save().unwrap();
        let loaded = Config::load();
        assert_eq!(loaded.cost_cap_usd, Some(0.50));
        assert_eq!(loaded.idle_timeout_secs, Some(45));
    }

    #[test]
    fn save_creates_parent_dir() {
        let _g = scoped();
        // A fresh temp dir without `.leek/` — save() must create it.
        let cfg = Config { cost_cap_usd: Some(1.0), ..Config::default() };
        cfg.save().unwrap();
        assert!(Config::path().exists());
        assert_eq!(Config::load().cost_cap_usd, Some(1.0));
    }

    #[test]
    fn malformed_file_degrades_to_default() {
        let _g = scoped();
        // Write garbage at the config path.
        let path = Config::path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        // load() must not panic — it warns and falls back to default.
        let cfg = Config::load();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn unknown_field_is_rejected_on_load() {
        // `deny_unknown_fields` keeps stale field names from silently
        // surviving in a user's config when we rename one.
        let _g = scoped();
        let path = Config::path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"max_cost_usd_per_turn": 1.0}"#).unwrap();
        // Same forgiving path as malformed JSON — warn + default.
        assert_eq!(Config::load(), Config::default());
    }

    #[test]
    fn merge_overwrites_only_present_fields() {
        let mut base = Config {
            cost_cap_usd: Some(0.5),
            idle_timeout_secs: Some(90),
            ..Config::default()
        };
        let patch = Config { cost_cap_usd: Some(1.5), ..Config::default() };
        base.merge(patch);
        assert_eq!(base.cost_cap_usd, Some(1.5));
        // The untouched field survives the merge.
        assert_eq!(base.idle_timeout_secs, Some(90));
    }

    #[test]
    fn merge_can_set_zero_disable() {
        // The "0 disables the cap" path: a PATCH with `0.0` must overwrite
        // a previous positive value — `merge` checks `is_some`, not `> 0`.
        let mut base = Config { cost_cap_usd: Some(2.0), ..Config::default() };
        base.merge(Config { cost_cap_usd: Some(0.0), ..Config::default() });
        assert_eq!(base.cost_cap_usd, Some(0.0));
    }

    #[test]
    fn validate_rejects_negative_cost_cap() {
        let cfg = Config { cost_cap_usd: Some(-1.0), ..Config::default() };
        let errs = cfg.validate().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].field, "cost_cap_usd");
    }

    #[test]
    fn validate_accepts_zero_cost_cap() {
        let cfg = Config { cost_cap_usd: Some(0.0), ..Config::default() };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_threshold_and_loop_n() {
        let cfg = Config {
            doom_loop_threshold: Some(1),
            auto_compact_threshold: Some(1.5),
            max_iterations: Some(0),
            context_window: Some(0),
            ..Config::default()
        };
        let errs = cfg.validate().unwrap_err();
        let fields: Vec<&str> = errs.iter().map(|e| e.field).collect();
        assert!(fields.contains(&"doom_loop_threshold"));
        assert!(fields.contains(&"auto_compact_threshold"));
        assert!(fields.contains(&"max_iterations"));
        assert!(fields.contains(&"context_window"));
    }

    // ── M3.6: ConfigPatch null=clear ──────────────────────────────────

    #[test]
    fn patch_absent_field_is_noop() {
        let patch: ConfigPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(patch.cost_cap_usd, None);

        let mut base = Config { cost_cap_usd: Some(2.0), ..Config::default() };
        base.merge_patch(patch);
        // Absent → leave alone.
        assert_eq!(base.cost_cap_usd, Some(2.0));
    }

    #[test]
    fn patch_explicit_null_clears_field() {
        // The whole reason ConfigPatch exists — the previous Config::merge
        // collapsed absent and null and dropped the user's "clear this"
        // intent on the floor.
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"cost_cap_usd": null}"#).unwrap();
        assert_eq!(patch.cost_cap_usd, Some(None));

        let mut base = Config { cost_cap_usd: Some(2.0), ..Config::default() };
        base.merge_patch(patch);
        // Explicit null → cleared back to None.
        assert_eq!(base.cost_cap_usd, None);
    }

    #[test]
    fn patch_value_sets_field() {
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"cost_cap_usd": 1.5}"#).unwrap();
        assert_eq!(patch.cost_cap_usd, Some(Some(1.5)));

        let mut base = Config::default();
        base.merge_patch(patch);
        assert_eq!(base.cost_cap_usd, Some(1.5));
    }

    #[test]
    fn patch_clears_multiple_fields_at_once() {
        // The "Reset to default" path: send `null` for every field.
        let patch: ConfigPatch = serde_json::from_str(
            r#"{
                "cost_cap_usd": null,
                "idle_timeout_secs": null,
                "builtin_url_abort_threshold": null
            }"#,
        )
        .unwrap();

        let mut base = Config {
            cost_cap_usd: Some(2.0),
            idle_timeout_secs: Some(45),
            builtin_url_abort_threshold: Some(15),
            // Untouched fields stay put.
            wall_clock_secs: Some(60),
            ..Config::default()
        };
        base.merge_patch(patch);
        assert_eq!(base.cost_cap_usd, None);
        assert_eq!(base.idle_timeout_secs, None);
        assert_eq!(base.builtin_url_abort_threshold, None);
        // Field not in the patch is left alone.
        assert_eq!(base.wall_clock_secs, Some(60));
    }

    #[test]
    fn patch_unknown_field_rejected_at_parse() {
        let res: Result<ConfigPatch, _> =
            serde_json::from_str(r#"{"max_cost_usd_per_turn": 1.0}"#);
        // deny_unknown_fields on ConfigPatch must fire — otherwise a
        // typo'd field would silently noop and confuse the user.
        assert!(res.is_err());
    }

    #[test]
    fn patch_validate_rejects_negative_cost_cap() {
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"cost_cap_usd": -1.0}"#).unwrap();
        let errs = patch.validate().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].field, "cost_cap_usd");
    }

    // ── M3.7: reasoning_effort validation ────────────────────────────

    #[test]
    fn validate_accepts_each_whitelisted_effort() {
        for effort in ["minimal", "low", "medium", "high", "xhigh"] {
            let cfg = Config {
                reasoning_effort: Some(effort.into()),
                ..Config::default()
            };
            assert!(
                cfg.validate().is_ok(),
                "effort '{effort}' must pass validation",
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_reasoning_effort() {
        let cfg = Config {
            reasoning_effort: Some("nuclear".into()),
            ..Config::default()
        };
        let errs = cfg.validate().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].field, "reasoning_effort");
    }

    #[test]
    fn patch_validate_rejects_unknown_reasoning_effort() {
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"reasoning_effort": "ultra"}"#).unwrap();
        let errs = patch.validate().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].field, "reasoning_effort");
    }

    #[test]
    fn patch_null_clears_reasoning_effort() {
        // Symmetric with the other null-clears tests above — the "Reset
        // to default" path must be able to drop a stored effort back to
        // None so the resolver re-applies the built-in default.
        let patch: ConfigPatch =
            serde_json::from_str(r#"{"reasoning_effort": null}"#).unwrap();
        assert_eq!(patch.reasoning_effort, Some(None));

        let mut base = Config {
            reasoning_effort: Some("xhigh".into()),
            ..Config::default()
        };
        base.merge_patch(patch);
        assert_eq!(base.reasoning_effort, None);
    }

    #[test]
    fn patch_validate_accepts_explicit_null() {
        // Clearing a field via explicit null is always valid — it sends
        // the resolver back to its built-in default, which is by
        // construction in range.
        let patch: ConfigPatch = serde_json::from_str(
            r#"{"cost_cap_usd": null, "max_iterations": null}"#,
        )
        .unwrap();
        assert!(patch.validate().is_ok());
    }

    #[test]
    fn config_dir_honors_env_override() {
        let _g = scoped();
        let dir = Config::config_dir();
        // Should match the override we set in `scoped()`.
        assert_eq!(
            dir,
            PathBuf::from(std::env::var("LEEK_CONFIG_DIR").unwrap())
        );
    }
}
