//! Scan the three agent discovery layers, parse `AGENT.md` files, build
//! an `AgentRegistry`. Errors on individual agents are warn + skip — a
//! malformed `AGENT.md` must never block server startup (same posture as
//! skill loading; M2.7 spec §C).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::agent_def::{AgentDef, AgentLayer, AgentRegistry};

#[derive(Debug)]
pub struct AgentLoadError {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for AgentLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

/// Built-in agents shipped with the repo (`harness/agents/`). Resolved
/// relative to `CARGO_MANIFEST_DIR` so the path holds whether the binary
/// is run from the repo root or installed via `cargo install`.
pub fn builtin_agents_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("harness")
        .join("agents")
}

/// `~/.leek/agents/`. Falls back to `./.leek-fallback/agents/` if `$HOME`
/// is unset (mostly to keep tests deterministic). Mirrors
/// `skills::user_skills_dir`.
pub fn user_agents_dir() -> PathBuf {
    expand_home("~/.leek/agents/")
}

/// Expand a leading `~` to `$HOME`. Delegated to the skills helper so
/// both subsystems agree on tilde semantics — there is only one home
/// convention in leek.
pub fn expand_home(path: &str) -> PathBuf {
    crate::skills::expand_home(path)
}

/// Load every agent from the three layers. Each `(dir, layer)` entry is
/// an AGENT-bearing directory. A missing dir is silent — every layer is
/// optional. Later entries in `layers` override earlier ones, so the
/// caller orders the slice low-priority first.
pub fn load_layered(layers: &[(&Path, AgentLayer)]) -> (AgentRegistry, Vec<AgentLoadError>) {
    let mut merged: HashMap<String, AgentDef> = HashMap::new();
    let mut errors: Vec<AgentLoadError> = Vec::new();

    for (dir, layer) in layers {
        if !dir.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                errors.push(AgentLoadError {
                    path: dir.to_path_buf(),
                    reason: format!("cannot read directory: {e}"),
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let agent_md = path.join("AGENT.md");
            if !agent_md.exists() {
                continue;
            }
            match load_one(&path, &agent_md, *layer) {
                Ok(agent) => {
                    merged.insert(agent.name.clone(), agent);
                }
                Err(e) => errors.push(e),
            }
        }
    }

    (AgentRegistry::new(merged), errors)
}

/// Load one `AGENT.md` into an `AgentDef`. The directory name is the
/// fallback for `name`; an invalid frontmatter or missing `description`
/// is an error (skip + warn — server still starts).
fn load_one(dir: &Path, agent_md: &Path, layer: AgentLayer) -> Result<AgentDef, AgentLoadError> {
    let raw = std::fs::read_to_string(agent_md).map_err(|e| AgentLoadError {
        path: agent_md.to_path_buf(),
        reason: format!("read failed: {e}"),
    })?;
    // Reuse the skill loader's YAML splitter — same SKILL.md / AGENT.md
    // file format, separate types and registries.
    let (frontmatter, body) = crate::skills::loader::split_frontmatter(&raw);

    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let name = frontmatter
        .get("name")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(dir_name);
    if name.is_empty() {
        return Err(AgentLoadError {
            path: agent_md.to_path_buf(),
            reason: "agent has no `name` and the parent directory has no name".into(),
        });
    }
    if !is_valid_agent_name(&name) {
        return Err(AgentLoadError {
            path: agent_md.to_path_buf(),
            reason: format!(
                "invalid agent name '{name}' — must match [A-Za-z0-9_-]{{1,64}}"
            ),
        });
    }

    let description = frontmatter
        .get("description")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if description.is_empty() {
        return Err(AgentLoadError {
            path: agent_md.to_path_buf(),
            reason: "missing required `description` frontmatter".into(),
        });
    }

    // Skills accept both `allowed-tools` and (per CC convention) we
    // accept either spelling for AGENT.md too — `allowed_tools` is the
    // form most users will write because Rust / Python uses underscores.
    let allowed_tools = frontmatter
        .get("allowed_tools")
        .or_else(|| frontmatter.get("allowed-tools"))
        .map(|s| parse_list(s))
        .unwrap_or_default();

    let model = frontmatter
        .get("model")
        .cloned()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // M3.6 §E: per-subagent cost cap (USD per loop). A bad value warns
    // and falls through to `None` (= inherit the main agent's cap) so
    // one bad frontmatter line doesn't take the subagent out of
    // service. `0` is preserved (the resolver treats it as "no cap").
    let cost_cap_usd = frontmatter.get("cost_cap_usd").and_then(|raw| {
        let s = raw.trim();
        if s.is_empty() {
            return None;
        }
        match s.parse::<f64>() {
            Ok(v) if v.is_finite() && v >= 0.0 => Some(v),
            _ => {
                tracing::warn!(
                    path = %agent_md.display(),
                    value = s,
                    "AGENT.md cost_cap_usd is not a non-negative finite number — ignoring"
                );
                None
            }
        }
    });

    let system_prompt = body.trim().to_string();
    if system_prompt.is_empty() {
        return Err(AgentLoadError {
            path: agent_md.to_path_buf(),
            reason: "AGENT.md body is empty — the body is the subagent's system prompt".into(),
        });
    }

    Ok(AgentDef {
        name,
        description,
        allowed_tools,
        system_prompt: system_prompt.into(),
        model,
        cost_cap_usd,
        source_layer: layer,
    })
}

/// `[A-Za-z0-9_-]{1,64}`. We allow uppercase to match the skill loader's
/// lenient rule and to keep AGENT.md / SKILL.md naming consistent.
fn is_valid_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parse a flat list value — `[a, b]` or a space-separated string,
/// since CC accepts both forms for `allowed_tools`. Mirrors
/// `skills::loader::parse_list`.
fn parse_list(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let body = if raw.starts_with('[') && raw.ends_with(']') {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    body.split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\''))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(label: &str) -> PathBuf {
        let unique = format!(
            "{}-{}-{}",
            label,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let p = std::env::temp_dir().join(format!("leek-agents-test-{unique}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_agent(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("AGENT.md"), body).unwrap();
    }

    #[test]
    fn empty_layers_yield_empty_registry() {
        let (reg, errs) = load_layered(&[]);
        assert!(reg.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn missing_layer_dir_is_silent() {
        let nonexistent = std::env::temp_dir().join("leek-agents-test-no-such-dir-xyz123");
        let _ = fs::remove_dir_all(&nonexistent);
        let (reg, errs) = load_layered(&[(&nonexistent, AgentLayer::Builtin)]);
        assert!(reg.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn loads_one_agent_with_full_frontmatter() {
        let root = temp_root("basic");
        write_agent(
            &root,
            "alpha",
            "---\nname: alpha\ndescription: A test agent\nallowed_tools: [corpus_search, corpus_read]\nmodel: gpt-5.5\n---\n\nYou are alpha.\n",
        );
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        assert!(errs.is_empty(), "errs: {errs:?}");
        let a = reg.get("alpha").unwrap();
        assert_eq!(a.name, "alpha");
        assert_eq!(a.description, "A test agent");
        assert_eq!(a.allowed_tools, vec!["corpus_search", "corpus_read"]);
        assert_eq!(a.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(a.system_prompt.as_ref(), "You are alpha.");
        assert_eq!(a.source_layer, AgentLayer::Builtin);
    }

    #[test]
    fn accepts_dashed_allowed_tools_spelling() {
        // CC parity: `allowed-tools` (dashed) and `allowed_tools` (snake)
        // must both work — `allowed-tools` is the spelling skill files
        // already use, and many users will copy from there.
        let root = temp_root("dashed-tools");
        write_agent(
            &root,
            "ag",
            "---\ndescription: d\nallowed-tools: [web_fetch]\n---\nbody",
        );
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::User)]);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(reg.get("ag").unwrap().allowed_tools, vec!["web_fetch"]);
    }

    #[test]
    fn agent_name_falls_back_to_directory_name() {
        let root = temp_root("dirname");
        write_agent(&root, "my-agent", "---\ndescription: dir-named\n---\nbody");
        let (reg, _) = load_layered(&[(root.as_path(), AgentLayer::User)]);
        let a = reg.get("my-agent").unwrap();
        assert_eq!(a.name, "my-agent");
    }

    #[test]
    fn missing_description_is_an_error_and_skip() {
        let root = temp_root("nodesc");
        write_agent(&root, "broken", "---\nname: broken\n---\nbody");
        write_agent(&root, "ok", "---\ndescription: fine\n---\nbody");
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        assert!(reg.get("ok").is_some());
        assert!(reg.get("broken").is_none());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("description"));
    }

    #[test]
    fn empty_body_is_an_error_and_skip() {
        let root = temp_root("nobody");
        // A subagent with no body has no system prompt — fail loud.
        write_agent(&root, "void", "---\ndescription: empty body\n---\n");
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        assert!(reg.is_empty());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("body"));
    }

    #[test]
    fn project_layer_overrides_user_overrides_builtin() {
        let builtin = temp_root("layer-b");
        let user = temp_root("layer-u");
        let project = temp_root("layer-p");
        write_agent(
            &builtin,
            "shared",
            "---\ndescription: from builtin\n---\nbuiltin body",
        );
        write_agent(
            &user,
            "shared",
            "---\ndescription: from user\n---\nuser body",
        );
        write_agent(
            &project,
            "shared",
            "---\ndescription: from project\n---\nproject body",
        );
        let (reg, errs) = load_layered(&[
            (builtin.as_path(), AgentLayer::Builtin),
            (user.as_path(), AgentLayer::User),
            (project.as_path(), AgentLayer::Project),
        ]);
        assert!(errs.is_empty());
        let a = reg.get("shared").unwrap();
        assert_eq!(a.source_layer, AgentLayer::Project);
        assert_eq!(a.system_prompt.as_ref(), "project body");
    }

    #[test]
    fn invalid_agent_name_rejected() {
        let root = temp_root("badname");
        write_agent(
            &root,
            "weird name with spaces",
            "---\ndescription: hi\n---\nbody",
        );
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        assert!(reg.is_empty());
        assert_eq!(errs.len(), 1);
        assert!(errs[0].reason.contains("invalid agent name"));
    }

    #[test]
    fn directory_without_agent_md_is_ignored() {
        let root = temp_root("noagentmd");
        std::fs::create_dir_all(root.join("noop")).unwrap();
        std::fs::write(root.join("noop/README.md"), "not an agent").unwrap();
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        assert!(reg.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn cost_cap_usd_frontmatter_loads() {
        // M3.6 §E: AGENT.md may carry an optional `cost_cap_usd` field
        // (USD per loop). Loader parses it into `AgentDef.cost_cap_usd`;
        // an absent field stays None (= inherit the main agent's cap).
        let root = temp_root("cost-cap");
        write_agent(
            &root,
            "capped",
            "---\ndescription: capped agent\ncost_cap_usd: 0.50\n---\nbody",
        );
        write_agent(&root, "uncapped", "---\ndescription: no cap\n---\nbody");
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(reg.get("capped").unwrap().cost_cap_usd, Some(0.50));
        assert_eq!(reg.get("uncapped").unwrap().cost_cap_usd, None);
    }

    #[test]
    fn cost_cap_usd_invalid_value_falls_through_to_none() {
        // Bad value warns + ignores — one typo must not take the
        // subagent out of service.
        let root = temp_root("cost-cap-bad");
        write_agent(
            &root,
            "bad",
            "---\ndescription: bad cost cap\ncost_cap_usd: not-a-number\n---\nbody",
        );
        write_agent(
            &root,
            "neg",
            "---\ndescription: negative cap\ncost_cap_usd: -1.0\n---\nbody",
        );
        let (reg, errs) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        // The agent still loads — only `cost_cap_usd` is dropped.
        assert!(errs.is_empty(), "agent should still load: {errs:?}");
        assert_eq!(reg.get("bad").unwrap().cost_cap_usd, None);
        assert_eq!(reg.get("neg").unwrap().cost_cap_usd, None);
    }

    #[test]
    fn shipped_builtin_agents_have_cost_caps() {
        // M3.6 §E ships per-subagent caps on every built-in worker so
        // a default-config user can rely on quick-screen never running
        // away into a multi-dollar tab. The values are spec-fixed
        // (dispatch §E): if any of them drift, both spec + this test
        // need an update.
        let dir = builtin_agents_dir();
        if !dir.exists() {
            return;
        }
        let (reg, _errs) = load_layered(&[(dir.as_path(), AgentLayer::Builtin)]);
        let expect: &[(&str, f64)] = &[
            ("quick-screen", 0.20),
            ("deep-review", 5.00),
            ("comparison", 3.00),
            ("general-purpose", 2.00),
            ("corpus-expert", 1.50),
        ];
        for (name, cap) in expect {
            let got = reg.get(name).and_then(|a| a.cost_cap_usd);
            assert_eq!(
                got,
                Some(*cap),
                "built-in {name} should carry cost_cap_usd = {cap}",
            );
        }
    }

    #[test]
    fn allowed_tools_defaults_to_empty_meaning_full_surface() {
        let root = temp_root("alltools");
        write_agent(
            &root,
            "general",
            "---\ndescription: full surface\n---\nbody.",
        );
        let (reg, _) = load_layered(&[(root.as_path(), AgentLayer::Builtin)]);
        // Empty vec — the subagent loop treats this as "full tool set".
        assert!(reg.get("general").unwrap().allowed_tools.is_empty());
    }

    #[test]
    fn skills_and_agents_share_no_state() {
        // Sanity: AgentLayer and SkillLayer are different types — the
        // compiler enforces strict separation. If this test compiles,
        // the only thing they share is the YAML splitter, which is fine.
        let a: AgentLayer = AgentLayer::Project;
        assert_eq!(a.as_str(), "project");
    }

    #[test]
    fn builtin_agents_dir_resolves_under_workspace() {
        // The path is built from `CARGO_MANIFEST_DIR`, so it should
        // resolve to an absolute on-disk location regardless of where
        // the tests are run from.
        let dir = builtin_agents_dir();
        assert!(dir.is_absolute());
        assert!(dir.ends_with("harness/agents"));
    }

    #[test]
    fn shipped_builtin_agents_load_cleanly() {
        // M2.7 spec §H ships `general-purpose` + `corpus-expert`; M3
        // adds three A-share research workers. The shipped files must
        // all parse without errors — if a frontmatter typo lands, this
        // test catches it before a user does.
        let dir = builtin_agents_dir();
        if !dir.exists() {
            // Skip when run from a slim checkout that doesn't include
            // the harness/ tree. Locally this branch is never taken.
            return;
        }
        let (reg, errs) = load_layered(&[(dir.as_path(), AgentLayer::Builtin)]);
        assert!(errs.is_empty(), "built-in AGENT.md errors: {errs:?}");
        let names = reg.names();
        for required in [
            "general-purpose",
            "corpus-expert",
            "quick-screen",
            "deep-review",
            "comparison",
        ] {
            assert!(
                names.contains(&required.to_string()),
                "missing built-in agent '{required}'; loaded: {names:?}"
            );
        }
        // corpus-expert ships with a restricted tool set; if a user
        // edits the file and forgets to keep the allow-list, this catches it.
        let ce = reg.get("corpus-expert").unwrap();
        assert!(ce.allowed_tools.contains(&"corpus_search".to_string()));
        assert!(ce.allowed_tools.contains(&"corpus_read".to_string()));
        // quick-screen restricts to the three A-share snapshot tools.
        let qs = reg.get("quick-screen").unwrap();
        for t in ["market_quote", "get_company_info", "get_capital_flow"] {
            assert!(
                qs.allowed_tools.contains(&t.to_string()),
                "quick-screen missing tool '{t}' in allow-list",
            );
        }
        // deep-review explicitly opts into the full tool surface — its
        // allowed_tools is empty (the loader's "empty = no restriction" rule).
        assert!(reg.get("deep-review").unwrap().allowed_tools.is_empty());
        // comparison spawns subagents and reads light data, so it owns
        // task + the cheap getters.
        let cmp = reg.get("comparison").unwrap();
        assert!(cmp.allowed_tools.contains(&"task".to_string()));
        assert!(cmp.allowed_tools.contains(&"market_quote".to_string()));
    }
}
