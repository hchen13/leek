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
    /// overwrites the corresponding field, `None` leaves it alone. This is
    /// the operation behind `PATCH /api/v1/settings`.
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
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
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

    /// Point `Config::path()` at a uuid-named scratch dir for the lifetime
    /// of one test. Returns the dir + a guard that restores the previous
    /// env value on drop, so parallel tests don't trample each other.
    struct ScopedConfigDir {
        _dir: tempdir::TempDir,
        prev: Option<String>,
    }
    impl Drop for ScopedConfigDir {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("LEEK_CONFIG_DIR", v),
                None => std::env::remove_var("LEEK_CONFIG_DIR"),
            }
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
        let dir = tempdir::TempDir::new("leek-config-test").unwrap();
        let prev = std::env::var("LEEK_CONFIG_DIR").ok();
        std::env::set_var("LEEK_CONFIG_DIR", dir.path());
        ScopedConfigDir { _dir: dir, prev }
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
