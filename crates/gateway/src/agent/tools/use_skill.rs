use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::llm::ToolSpec;

use super::ToolHandler;

const TOOL_NAME: &str = "use_skill";
const DEFAULT_SKILLS_ROOT: &str = "harness/skills";
const ENV_SKILLS_ROOT: &str = "LEEK_SKILLS_ROOT";

pub struct UseSkillTool {
    registry: &'static SkillRegistry,
}

impl UseSkillTool {
    pub fn new() -> Self {
        Self {
            registry: default_skill_registry(),
        }
    }
}

impl Default for UseSkillTool {
    fn default() -> Self {
        Self::new()
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
            description: "Load a portable research methodology by name. Skills describe how to \
                think about a class of problem (what evidence matters, how to reason under \
                uncertainty, what a useful deliverable contains); they are not runtime \
                checklists and do not seed the active plan. Skills are loaded from \
                harness/skills at runtime; edits to SKILL.md take effect on the next call \
                without restarting the service. The agent decides how the methodology maps \
                onto available tools and how to shape the active plan."
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

        self.registry.load_skill(name)
    }
}

pub fn skill_metadata() -> String {
    default_skill_registry().metadata_prompt()
}

fn default_skill_registry() -> &'static SkillRegistry {
    static REGISTRY: OnceLock<SkillRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| SkillRegistry::new(resolve_skills_root()))
}

fn resolve_skills_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(root) = std::env::var(ENV_SKILLS_ROOT) {
        candidates.push(PathBuf::from(root));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(DEFAULT_SKILLS_ROOT));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DEFAULT_SKILLS_ROOT),
    );
    candidates
        .into_iter()
        .find(|p| p.join("equity-valuation").join("SKILL.md").exists())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SKILLS_ROOT))
}

struct SkillRegistry {
    root: PathBuf,
    dirty: std::sync::Arc<AtomicBool>,
    cache: Mutex<Option<SkillCatalog>>,
    _watcher: Option<RecommendedWatcher>,
}

#[derive(Clone, Default)]
struct SkillCatalog {
    skills: BTreeMap<String, SkillDoc>,
}

#[derive(Clone)]
struct SkillDoc {
    name: String,
    description: String,
    body: String,
}

impl SkillRegistry {
    fn new(root: PathBuf) -> Self {
        let dirty = std::sync::Arc::new(AtomicBool::new(true));
        let watcher = start_skill_watcher(&root, dirty.clone());
        Self {
            root,
            dirty,
            cache: Mutex::new(None),
            _watcher: watcher,
        }
    }

    fn metadata_prompt(&self) -> String {
        match self.catalog() {
            Ok(catalog) => catalog.metadata_prompt(),
            Err(err) => {
                warn!(error = %err, "failed to load skill metadata");
                "- **equity-valuation**: A股或美股的系统性估值分析。\n- **crypto-research**: 加密货币/代币系统性投研。".to_string()
            }
        }
    }

    fn load_skill(&self, name: &str) -> Result<String> {
        let catalog = self.catalog()?;
        let Some(doc) = catalog.skills.get(name) else {
            let available = catalog
                .skills
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            bail!("unknown skill '{name}'. Available: {available}");
        };
        Ok(doc.body.trim().to_string())
    }

    fn catalog(&self) -> Result<SkillCatalog> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow!("skill registry cache lock poisoned"))?;
        if self.dirty.load(Ordering::Acquire) || cache.is_none() {
            let catalog = load_skill_catalog(&self.root)
                .with_context(|| format!("loading skills from {}", self.root.display()))?;
            *cache = Some(catalog);
            self.dirty.store(false, Ordering::Release);
        }
        Ok(cache.clone().unwrap_or_default())
    }
}

impl SkillCatalog {
    fn metadata_prompt(&self) -> String {
        self.skills
            .values()
            .map(|doc| {
                if doc.description.trim().is_empty() {
                    format!("- **{}**: Specialized research workflow.", doc.name)
                } else {
                    format!("- **{}**: {}", doc.name, doc.description.trim())
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn start_skill_watcher(
    root: &Path,
    dirty: std::sync::Arc<AtomicBool>,
) -> Option<RecommendedWatcher> {
    let root = root.to_path_buf();
    if !root.exists() {
        return None;
    }
    let watcher_dirty = dirty.clone();
    let mut watcher =
        match notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                let relevant = event.paths.iter().any(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name == "SKILL.md")
                        .unwrap_or(false)
                        || path.is_dir()
                });
                if relevant {
                    watcher_dirty.store(true, Ordering::Release);
                }
            }
            Err(err) => warn!(error = %err, "skill watcher event error"),
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                warn!(error = %err, root = %root.display(), "failed to start skill watcher");
                return None;
            }
        };
    if let Err(err) = watcher.watch(&root, RecursiveMode::Recursive) {
        warn!(error = %err, root = %root.display(), "failed to watch skill root");
        return None;
    }
    Some(watcher)
}

fn load_skill_catalog(root: &Path) -> Result<SkillCatalog> {
    let mut skills = BTreeMap::new();
    if !root.exists() {
        return Ok(SkillCatalog { skills });
    }
    for entry in std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_path = path.join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&skill_path)
            .with_context(|| format!("reading {}", skill_path.display()))?;
        let dir_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let doc = parse_skill_doc(&raw, &dir_name)?;
        skills.insert(doc.name.clone(), doc);
    }
    Ok(SkillCatalog { skills })
}

fn parse_skill_doc(raw: &str, fallback_name: &str) -> Result<SkillDoc> {
    let (frontmatter, body) = split_frontmatter(raw);
    let metadata = frontmatter
        .map(|yaml| serde_yaml::from_str::<serde_yaml::Value>(yaml))
        .transpose()
        .context("parsing skill frontmatter")?;
    let name = metadata
        .as_ref()
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let description = metadata
        .as_ref()
        .and_then(|v| v.get("description"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    Ok(SkillDoc {
        name,
        description,
        body: body.trim().to_string(),
    })
}

fn split_frontmatter(s: &str) -> (Option<&str>, &str) {
    let s = s.trim_start();
    if !s.starts_with("---") {
        return (None, s);
    }
    let after_open = &s[3..];
    if let Some(close) = after_open.find("\n---") {
        let frontmatter = &after_open[..close];
        let body = &after_open[close + 4..];
        (Some(frontmatter), body)
    } else {
        (None, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_yaml_header() {
        let s = "---\nname: test\ndescription: foo\n---\n\n# Body\n\ncontent here";
        let (_, result) = split_frontmatter(s);
        assert!(!result.contains("name: test"));
        assert!(result.contains("# Body"));
    }

    #[test]
    fn strip_frontmatter_passthrough_no_header() {
        let s = "# No frontmatter\n\nsome content";
        assert_eq!(split_frontmatter(s), (None, s));
    }

    #[test]
    fn known_skills_load_without_panic() {
        let catalog = load_skill_catalog(&resolve_skills_root()).unwrap();
        assert!(catalog.skills.contains_key("equity-valuation"));
        assert!(catalog.skills.contains_key("crypto-research"));
    }

    #[test]
    fn strip_equity_skill_frontmatter() {
        let doc = load_skill_catalog(&resolve_skills_root())
            .unwrap()
            .skills
            .remove("equity-valuation")
            .unwrap();
        assert!(doc.body.starts_with('#'));
        assert!(!doc.body.contains("name: equity-valuation"));
    }

    #[test]
    fn equity_skill_is_portable_methodology() {
        // Skill body must not name Leek runtime tools — that mapping belongs in
        // the harness, not in portable methodology. If you add new Leek tools,
        // do NOT push their names back into the skill body.
        let doc = load_skill_catalog(&resolve_skills_root())
            .unwrap()
            .skills
            .remove("equity-valuation")
            .unwrap();
        for forbidden in [
            "update_plan",
            "corpus_search",
            "web_search",
            "web_fetch",
            "market_quote",
            "get_financials",
            "get_candlesticks",
            "get_capital_flow",
            "get_company_info",
            "sec_filing_fetch",
        ] {
            assert!(
                !doc.body.contains(forbidden),
                "equity-valuation skill must not name Leek tool `{forbidden}`; runtime mapping belongs in the harness"
            );
        }
    }

    #[test]
    fn registry_reloads_after_dirty() {
        let root = std::env::temp_dir().join(format!("leek-skills-{}", uuid::Uuid::new_v4()));
        let skill_dir = root.join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: old\n---\n\n# Old",
        )
        .unwrap();
        let registry = SkillRegistry::new(root.clone());
        assert!(registry.metadata_prompt().contains("old"));
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: new\n---\n\n# New",
        )
        .unwrap();
        registry.dirty.store(true, Ordering::Release);
        assert!(registry.metadata_prompt().contains("new"));
        assert_eq!(registry.load_skill("demo").unwrap(), "# New");
        std::fs::remove_dir_all(root).unwrap();
    }
}
