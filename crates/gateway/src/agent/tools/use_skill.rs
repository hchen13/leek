use anyhow::{bail, Result};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "use_skill";

const SKILL_EQUITY_VALUATION: &str =
    include_str!("../../../../../harness/skills/equity-valuation/SKILL.md");
const SKILL_CRYPTO_RESEARCH: &str =
    include_str!("../../../../../harness/skills/crypto-research/SKILL.md");

pub struct UseSkillTool;

#[async_trait]
impl ToolHandler for UseSkillTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            name: TOOL_NAME.into(),
            description: "Load the full step-by-step workflow for a named research skill. \
                Call this as your FIRST action when the task matches a skill. \
                Returns the complete checklist you must follow."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name. Available: equity-valuation, crypto-research"
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
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("'name' is required"))?;

        let content = match name {
            "equity-valuation" => SKILL_EQUITY_VALUATION,
            "crypto-research" => SKILL_CRYPTO_RESEARCH,
            other => bail!("unknown skill '{other}'. Available: equity-valuation, crypto-research"),
        };

        // Strip YAML frontmatter (--- ... ---) before returning to LLM.
        let body = strip_frontmatter(content);
        Ok(body.trim().to_string())
    }
}

fn strip_frontmatter(s: &str) -> &str {
    let s = s.trim_start();
    if !s.starts_with("---") {
        return s;
    }
    // Find the closing ---
    let after_open = &s[3..];
    if let Some(close) = after_open.find("\n---") {
        &after_open[close + 4..]
    } else {
        s
    }
}

/// Metadata injected into the system prompt (name + one-line when-to-use).
pub const SKILL_METADATA: &str = "\
- **equity-valuation**: A股或美股的系统性估值分析。用户问\"值不值\"、\"估值\"、\"目标价\"、\"内在价值\"、\"DCF\"、\"有没有投资价值\"时立刻调用。
- **crypto-research**: 加密货币/代币系统性投研，或 meme/alpha 市场机会扫描。\
  用户分析具体代币时调用（单项目模式）；\
  用户问\"有哪些 meme 机会\"、\"扫一下 solana/binance alpha\"、\"有没有值得看的新项目\"时也调用（扫描模式）。";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_yaml_header() {
        let s = "---\nname: test\ndescription: foo\n---\n\n# Body\n\ncontent here";
        let result = strip_frontmatter(s);
        assert!(!result.contains("name: test"));
        assert!(result.contains("# Body"));
    }

    #[test]
    fn strip_frontmatter_passthrough_no_header() {
        let s = "# No frontmatter\n\nsome content";
        assert_eq!(strip_frontmatter(s), s);
    }

    #[test]
    fn known_skills_load_without_panic() {
        assert!(!SKILL_EQUITY_VALUATION.is_empty());
        assert!(!SKILL_CRYPTO_RESEARCH.is_empty());
    }

    #[test]
    fn strip_equity_skill_frontmatter() {
        let body = strip_frontmatter(SKILL_EQUITY_VALUATION).trim();
        assert!(body.starts_with('#'));
        assert!(!body.contains("name: equity-valuation"));
    }
}
