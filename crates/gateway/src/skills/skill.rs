//! `Skill` value + the `SkillRegistry` shared across requests.

use std::collections::HashMap;
use std::sync::Arc;

/// Which discovery layer a skill came from. The layer drives override
/// precedence (`Project` wins over `User` wins over `Builtin`) and is
/// surfaced via `GET /api/v1/skills` so the workbench can show provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillLayer {
    Builtin,
    User,
    Project,
}

impl SkillLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillLayer::Builtin => "builtin",
            SkillLayer::User => "user",
            SkillLayer::Project => "project",
        }
    }
}

/// A loaded skill — frontmatter + body, fully resolved at startup. Cheap to
/// clone (the body is shared as an `Arc<str>`).
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name — `name` frontmatter or, if absent, the parent directory.
    /// `[a-z0-9-]`, max 64 chars (CC convention).
    pub name: String,
    /// One-liner used in the system-prompt index. The skill body is hidden
    /// until the model calls `use_skill`.
    pub description: String,
    /// Optional pre-approved tool list (advisory in leek — surfaced to the
    /// model in the skill body header, not enforced as a set-intersection).
    pub allowed_tools: Vec<String>,
    /// Markdown body — returned by `use_skill`.
    pub body: Arc<str>,
    /// Discovery layer this skill was loaded from.
    pub source_layer: SkillLayer,
    /// `true` → not advertised in the system-prompt index; the model cannot
    /// auto-invoke it, but `use_skill(name)` still works (CC parity).
    pub disable_model_invocation: bool,
}

impl Skill {
    /// One-line index entry for the system prompt. Skills whose
    /// `disable_model_invocation` is set are filtered out **before** this
    /// is called — the index never sees them.
    pub fn index_line(&self) -> String {
        format!("- `{}` — {}", self.name, self.description.trim())
    }
}

/// In-memory skill registry — read-only after startup, shared across
/// request handlers and agent turns via `Arc`.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    /// `name → Skill`, post-override (one entry per skill name).
    inner: Arc<HashMap<String, Skill>>,
}

impl SkillRegistry {
    pub fn new(map: HashMap<String, Skill>) -> Self {
        Self {
            inner: Arc::new(map),
        }
    }

    /// `name → Skill`, post-override. The map is keyed by the resolved
    /// skill name (frontmatter or directory).
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.inner.get(name)
    }

    /// Iterate every skill (post-override). Stable order by name — handy
    /// for the system-prompt index, where deterministic order keeps the
    /// prompt KV-cache stable across restarts.
    pub fn iter_sorted(&self) -> Vec<&Skill> {
        let mut v: Vec<_> = self.inner.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Only the skills the **model** can see (`disable_model_invocation`
    /// filtered out). Used for the system-prompt index.
    pub fn model_visible(&self) -> Vec<&Skill> {
        self.iter_sorted()
            .into_iter()
            .filter(|s| !s.disable_model_invocation)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
