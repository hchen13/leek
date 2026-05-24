//! `AgentDef` value + the `AgentRegistry` shared across requests.

use std::collections::HashMap;
use std::sync::Arc;

/// Which discovery layer an agent came from. Same semantics as
/// `SkillLayer` (project > user > built-in), but kept as its own enum so
/// agents and skills stay strictly separate (ARCHITECTURE §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLayer {
    Builtin,
    User,
    Project,
}

impl AgentLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentLayer::Builtin => "builtin",
            AgentLayer::User => "user",
            AgentLayer::Project => "project",
        }
    }
}

/// A loaded subagent definition — frontmatter + markdown body, fully
/// resolved at startup. Cheap to clone (the body is shared as an
/// `Arc<str>`).
#[derive(Debug, Clone)]
pub struct AgentDef {
    /// Agent name — `name` frontmatter or the parent directory name.
    /// Validated against `[a-z0-9_-]{1,64}`.
    pub name: String,
    /// One-liner used in the system-prompt `task` tool description and in
    /// `GET /api/v1/agents`. Required.
    pub description: String,
    /// Optional pre-approved tool list. Unlike skills (where it is
    /// advisory), for agents this **collapses** the dispatchable tool set
    /// inside the subagent loop: a subagent can only call tools whose
    /// name appears here. An empty list means "all tools" (full surface).
    pub allowed_tools: Vec<String>,
    /// Markdown body — used verbatim as the subagent's system prompt
    /// (the parent agent's five-section prompt is NOT prepended).
    pub system_prompt: Arc<str>,
    /// Optional `model` override. The codex backend only exposes a single
    /// model in this milestone, so this field is recorded for parity with
    /// CC's `subagents.<name>.model` field but the subagent loop ignores
    /// it (the model is still the constant in `agent::mod`).
    pub model: Option<String>,
    /// Discovery layer this agent was loaded from.
    pub source_layer: AgentLayer,
}

/// In-memory subagent registry — read-only after startup, shared across
/// request handlers and the task tool via `Arc`.
#[derive(Debug, Clone, Default)]
pub struct AgentRegistry {
    /// `name → AgentDef`, post-override (one entry per agent name).
    inner: Arc<HashMap<String, AgentDef>>,
}

impl AgentRegistry {
    pub fn new(map: HashMap<String, AgentDef>) -> Self {
        Self {
            inner: Arc::new(map),
        }
    }

    /// `name → AgentDef`, post-override.
    pub fn get(&self, name: &str) -> Option<&AgentDef> {
        self.inner.get(name)
    }

    /// Iterate every agent (post-override). Stable alphabetical order so
    /// `GET /api/v1/agents` and the task-tool description stay
    /// reproducible across restarts.
    pub fn iter_sorted(&self) -> Vec<&AgentDef> {
        let mut v: Vec<_> = self.inner.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.iter_sorted().into_iter().map(|a| a.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(name: &str, layer: AgentLayer) -> AgentDef {
        AgentDef {
            name: name.into(),
            description: format!("desc for {name}"),
            allowed_tools: vec![],
            system_prompt: "system prompt".into(),
            model: None,
            source_layer: layer,
        }
    }

    #[test]
    fn registry_iter_sorted_is_alpha() {
        let mut m = HashMap::new();
        m.insert("zeta".into(), mk("zeta", AgentLayer::Builtin));
        m.insert("alpha".into(), mk("alpha", AgentLayer::Builtin));
        m.insert("mike".into(), mk("mike", AgentLayer::User));
        let reg = AgentRegistry::new(m);
        let names = reg.names();
        assert_eq!(names, vec!["alpha", "mike", "zeta"]);
    }

    #[test]
    fn registry_get_returns_loaded_agent() {
        let mut m = HashMap::new();
        m.insert("alpha".into(), mk("alpha", AgentLayer::Project));
        let reg = AgentRegistry::new(m);
        let got = reg.get("alpha").unwrap();
        assert_eq!(got.name, "alpha");
        assert_eq!(got.source_layer, AgentLayer::Project);
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn layer_as_str_matches_enum_name() {
        assert_eq!(AgentLayer::Builtin.as_str(), "builtin");
        assert_eq!(AgentLayer::User.as_str(), "user");
        assert_eq!(AgentLayer::Project.as_str(), "project");
    }
}
