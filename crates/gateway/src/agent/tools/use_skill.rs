//! `use_skill` — lazy-load a research skill's full body into the turn.
//!
//! Mirrors the Claude Code / Codex pattern of progressive skill disclosure:
//! the system prompt lists each skill's frontmatter `description` so the
//! model knows the skill exists, but the *body* of `harness/skills/<name>/SKILL.md`
//! is only loaded into context when the model explicitly calls
//! `use_skill(name)`. This keeps the per-turn token cost bounded as the
//! skill catalog grows.
//!
//! Phase 0 implementation. Future milestones (M2.5) will:
//! - widen discovery to user (`~/.leek/skills/`) and project
//!   (`<project>/.leek/skills/`) directories;
//! - watch the skill directories for changes (hot reload);
//! - support frontmatter fields beyond `name` / `description`
//!   (e.g. `allowed_tools`, `paths`, `disable-model-invocation`).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "use_skill";

pub struct UseSkillTool;

impl UseSkillTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHandler for UseSkillTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description:
                "Load the full body of a named research skill (a methodology framework) into your context. \
                 The system prompt lists each available skill with a one-line description; pass that skill's \
                 `name` here when the current task matches and you want the full methodology before doing \
                 analysis. Returns the skill's markdown body."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name as listed in the system prompt's `# Research Skills` section, e.g. `equity-valuation`."
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _cancel: CancellationToken,
        _ctx: &super::ToolContext,
    ) -> Result<String> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing 'name' argument"))?
            .trim();
        if name.is_empty() {
            return Err(anyhow!("'name' must not be empty"));
        }
        // Validate against the discovered list so a typo or hallucinated
        // name returns a clear error instead of a vague file-not-found.
        let known = crate::agent::harness::list_skill_names();
        if !known.iter().any(|s| s == name) {
            return Err(anyhow!(
                "unknown skill `{name}`. Available skills: {}.",
                known.join(", ")
            ));
        }
        let path = skill_path(name)
            .ok_or_else(|| anyhow!("could not resolve workspace root to load skill `{name}`"))?;
        let raw = fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow!("reading {}: {}", path.display(), e))?;
        Ok(strip_frontmatter(&raw))
    }
}

fn skill_path(name: &str) -> Option<PathBuf> {
    crate::agent::harness::workspace_root()
        .map(|root| root.join("harness").join("skills").join(name).join("SKILL.md"))
}

/// Strip a leading `---\n...\n---\n` YAML frontmatter block. Returns the
/// remaining body unchanged. If no frontmatter is present, returns the
/// input as-is.
fn strip_frontmatter(s: &str) -> String {
    let trimmed = s.trim_start_matches('\u{FEFF}'); // tolerate BOM
    if !trimmed.starts_with("---") {
        return trimmed.to_string();
    }
    // Skip the opening `---` line.
    let after_open = match trimmed.find('\n') {
        Some(i) => &trimmed[i + 1..],
        None => return String::new(),
    };
    // Find the closing `---` on its own line.
    if let Some(idx) = after_open.find("\n---") {
        let rest = &after_open[idx + 4..];
        // Drop any trailing newline left after the closing fence.
        return rest.trim_start_matches('\n').to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_yaml_block() {
        let s = "---\nname: foo\ndescription: bar\n---\n\n# Body\n\ncontent\n";
        let out = strip_frontmatter(s);
        assert!(out.starts_with("# Body"));
        assert!(!out.contains("name: foo"));
    }

    #[test]
    fn strip_frontmatter_passes_through_bodies_without_block() {
        let s = "# Body\n\ncontent";
        assert_eq!(strip_frontmatter(s), s);
    }

    #[test]
    fn strip_frontmatter_handles_bom() {
        let s = "\u{FEFF}---\nname: foo\n---\nbody";
        let out = strip_frontmatter(s);
        assert_eq!(out, "body");
    }
}
