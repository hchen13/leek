//! Hook config parser — both `~/.leek/config.json` (user) and per-plugin
//! `hooks/hooks.json`. Both files expose the same `{ "hooks": { ... } }`
//! shape (CC parity, M2.5 research notes §3).
//!
//! The CC config nests one extra level — each event maps to an array of
//! `{ matcher, hooks: [...] }`, where the inner `hooks` array is the list
//! of actual handlers. We mirror that structure verbatim.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::outcome::HookEvent;

/// One concrete hook definition (CC `type: "command"` only — `http`,
/// `mcp_tool`, `prompt`, `agent` are not implemented this milestone).
#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    /// `type` field in the CC schema. Defaults to `"command"`; unknown
    /// types are silently treated as command (the loader has already
    /// warned for them).
    #[serde(default = "default_type", rename = "type")]
    pub kind: String,
    /// Shell command to run via `sh -c <command>`. **Required.**
    pub command: String,
    /// Per-hook timeout in seconds; defaults to 60 (research notes §5).
    /// CC default is higher (600) but leek runs hooks inline with the
    /// agent loop, so we keep them snappy.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_type() -> String {
    "command".into()
}

fn default_timeout_secs() -> u64 {
    60
}

/// A `(matcher, hooks)` entry under one event.
#[derive(Debug, Clone, Deserialize)]
pub struct HookMatcher {
    /// CC matcher syntax. Empty / missing → match any. We support exact
    /// match and pipe-separated alternatives only; regex is not in scope
    /// (research notes §3).
    #[serde(default)]
    pub matcher: Option<String>,
    /// Handlers run in declaration order; the first to block wins.
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
}

impl HookMatcher {
    /// Does this matcher fire for the given target string (typically a
    /// tool name)? Empty / `*` matches anything.
    pub fn matches(&self, target: &str) -> bool {
        match self.matcher.as_deref() {
            None | Some("") | Some("*") => true,
            Some(pat) => pat.split('|').any(|s| s.trim() == target),
        }
    }
}

/// One full hook config file. The on-disk format keeps the CC nesting:
/// outer `hooks` → event → array of `{ matcher, hooks: [...] }`.
#[derive(Debug, Clone, Default, Deserialize)]
struct HookFile {
    #[serde(default)]
    hooks: HashMap<String, Vec<HookMatcher>>,
    // Any other settings keys in the file are tolerated (research notes
    // §3: leek reuses ~/.leek/config.json which may carry M2.6 settings).
}

/// Merged hook registry — what the engine actually queries at runtime.
/// Multiple config sources (user + plugins) merge into one of these.
#[derive(Debug, Clone, Default)]
pub struct HookSet {
    /// `event → list of matcher entries`. Within an event, matchers are
    /// in source order: user first, then each plugin as added.
    pub by_event: HashMap<HookEvent, Vec<HookMatcher>>,
}

impl HookSet {
    /// All `(matcher, hook)` pairs for `event` whose matcher fires for
    /// `target`. Returned in declaration order — the engine respects it
    /// when checking for a block verdict.
    pub fn matching<'a>(
        &'a self,
        event: HookEvent,
        target: &str,
    ) -> Vec<(&'a HookMatcher, &'a HookConfig)> {
        let mut out = Vec::new();
        if let Some(entries) = self.by_event.get(&event) {
            for entry in entries {
                if entry.matches(target) {
                    for h in &entry.hooks {
                        out.push((entry, h));
                    }
                }
            }
        }
        out
    }

    /// `true` iff at least one handler is registered for `event`.
    pub fn has_event(&self, event: HookEvent) -> bool {
        self.by_event
            .get(&event)
            .map(|v| v.iter().any(|m| !m.hooks.is_empty()))
            .unwrap_or(false)
    }

    /// Distinct events with at least one handler. Used for the startup
    /// log line ("hooks for N events").
    pub fn event_count(&self) -> usize {
        self.by_event
            .iter()
            .filter(|(_, v)| v.iter().any(|m| !m.hooks.is_empty()))
            .count()
    }

    /// Splice another set onto this one. Plugin-supplied hooks run after
    /// user-supplied ones within the same event (research notes §6).
    pub fn merge(&mut self, other: HookSet) {
        for (event, entries) in other.by_event {
            self.by_event.entry(event).or_default().extend(entries);
        }
    }
}

/// Read the user-level config (`~/.leek/config.json`) and pull out its
/// `hooks` section. A missing file is *not* an error — most users won't
/// have configured hooks. Malformed JSON is an error so the user gets
/// told (logged via the caller).
pub fn load_user_hooks(path: &Path) -> std::io::Result<HookSet> {
    if !path.exists() {
        return Ok(HookSet::default());
    }
    let raw = std::fs::read_to_string(path)?;
    parse_hooks_json(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parsing {} failed: {e}", path.display()),
        )
    })
}

/// Read a plugin's `hooks/hooks.json`. Same format as the user config,
/// just minus other settings keys (plugins only contribute hooks).
pub fn load_hooks_file(path: &Path) -> std::io::Result<HookSet> {
    let raw = std::fs::read_to_string(path)?;
    parse_hooks_json(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parsing {} failed: {e}", path.display()),
        )
    })
}

/// Parse a raw hooks-bearing JSON string. Unknown event names are
/// dropped with a warn log; everything else maps cleanly.
fn parse_hooks_json(raw: &str) -> Result<HookSet, serde_json::Error> {
    let file: HookFile = serde_json::from_str(raw)?;
    let mut set = HookSet::default();
    for (event_name, entries) in file.hooks {
        match HookEvent::from_name(&event_name) {
            Some(e) => {
                set.by_event.entry(e).or_default().extend(entries);
            }
            None => {
                tracing::warn!(event = %event_name, "unknown hook event name — skipping");
            }
        }
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_wildcard_matches_anything() {
        let m = HookMatcher {
            matcher: None,
            hooks: vec![],
        };
        assert!(m.matches("anything"));
        assert!(m.matches(""));
    }

    #[test]
    fn matcher_star_matches_anything() {
        let m = HookMatcher {
            matcher: Some("*".into()),
            hooks: vec![],
        };
        assert!(m.matches("any"));
    }

    #[test]
    fn matcher_exact_string() {
        let m = HookMatcher {
            matcher: Some("web_fetch".into()),
            hooks: vec![],
        };
        assert!(m.matches("web_fetch"));
        assert!(!m.matches("web_search"));
    }

    #[test]
    fn matcher_pipe_separated_alternates() {
        let m = HookMatcher {
            matcher: Some("web_fetch|corpus_search".into()),
            hooks: vec![],
        };
        assert!(m.matches("web_fetch"));
        assert!(m.matches("corpus_search"));
        assert!(!m.matches("update_plan"));
    }

    #[test]
    fn parses_full_config_with_matchers() {
        let raw = r#"
        {
          "hooks": {
            "PreToolUse": [
              {
                "matcher": "web_fetch",
                "hooks": [{ "type": "command", "command": "echo blocked", "timeout_secs": 5 }]
              }
            ],
            "Stop": [
              { "hooks": [{ "type": "command", "command": "true" }] }
            ]
          }
        }
        "#;
        let set = parse_hooks_json(raw).unwrap();
        assert!(set.has_event(HookEvent::PreToolUse));
        assert!(set.has_event(HookEvent::Stop));
        assert_eq!(set.event_count(), 2);
        let m = &set.by_event[&HookEvent::PreToolUse][0];
        assert_eq!(m.matcher.as_deref(), Some("web_fetch"));
        assert_eq!(m.hooks[0].timeout_secs, 5);
    }

    #[test]
    fn unknown_event_name_skipped() {
        // `WeirdEvent` is skipped (not in the known set) so it never makes
        // it into the registry. `Stop` is known but its handler list is
        // empty, so it's also not surfaced via `event_count` / `has_event`.
        let raw = r#"{ "hooks": { "WeirdEvent": [], "Stop": [] } }"#;
        let set = parse_hooks_json(raw).unwrap();
        assert!(!set.has_event(HookEvent::Stop));
        assert_eq!(set.event_count(), 0);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let nope = std::env::temp_dir().join("leek-hooks-nonexistent-xxx.json");
        let _ = std::fs::remove_file(&nope);
        let set = load_user_hooks(&nope).unwrap();
        assert_eq!(set.event_count(), 0);
    }

    #[test]
    fn tolerates_extra_keys_in_config_file() {
        // `~/.leek/config.json` may carry settings beyond `hooks`.
        let raw = r#"
        {
          "cost_cap_usd": 5.0,
          "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "true" }] }]
          }
        }
        "#;
        let set = parse_hooks_json(raw).unwrap();
        assert!(set.has_event(HookEvent::Stop));
    }

    #[test]
    fn default_timeout_is_60s() {
        let raw = r#"{ "hooks": { "Stop": [{ "hooks": [{ "command": "true" }] }] } }"#;
        let set = parse_hooks_json(raw).unwrap();
        let h = &set.by_event[&HookEvent::Stop][0].hooks[0];
        assert_eq!(h.timeout_secs, 60);
        assert_eq!(h.kind, "command");
    }

    #[test]
    fn merge_appends_matchers_within_same_event() {
        let raw_a = r#"{ "hooks": { "PreToolUse": [{ "matcher": "a", "hooks": [{ "command": "x" }] }] } }"#;
        let raw_b = r#"{ "hooks": { "PreToolUse": [{ "matcher": "b", "hooks": [{ "command": "y" }] }] } }"#;
        let mut a = parse_hooks_json(raw_a).unwrap();
        let b = parse_hooks_json(raw_b).unwrap();
        a.merge(b);
        let entries = &a.by_event[&HookEvent::PreToolUse];
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].matcher.as_deref(), Some("a"));
        assert_eq!(entries[1].matcher.as_deref(), Some("b"));
    }

    #[test]
    fn matching_returns_only_firing_handlers() {
        let raw = r#"
        {
          "hooks": {
            "PreToolUse": [
              { "matcher": "web_fetch",   "hooks": [{ "command": "1" }] },
              { "matcher": "*",           "hooks": [{ "command": "2" }] },
              { "matcher": "corpus_read", "hooks": [{ "command": "3" }] }
            ]
          }
        }
        "#;
        let set = parse_hooks_json(raw).unwrap();
        let matches = set.matching(HookEvent::PreToolUse, "web_fetch");
        let cmds: Vec<&str> = matches.iter().map(|(_, h)| h.command.as_str()).collect();
        assert_eq!(cmds, vec!["1", "2"]);
    }
}
