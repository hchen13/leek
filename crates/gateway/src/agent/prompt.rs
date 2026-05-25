//! System-prompt builder — assembled most-stable-first
//! (ARCHITECTURE §4.1, best KV-cache reuse).
//!
//! Order (M2.5):
//!   IDENTITY  →  OPERATING  →  SEEK_VERIFICATION_DISCIPLINE  →
//!   CORPUS_ORIENTATION  →  # 可用工具  →  # 可用 Skills
//!
//! The identity (who leek is) is fixed text. The operating note (how the
//! loop works) is short and tactical. The verification-discipline section
//! is leek's invariant: facts get searched before they get answered. The
//! corpus orientation tells the model how to think about its
//! knowledge-base — what corpus is for, how to query it. The tool roster
//! is next (the model also gets each tool's full schema via the API tools
//! array — so prompt prose is just orientation, not the source of truth).
//!
//! Last is the skill index: skills are softer than tools — opt-in, lazy
//! recipes the model loads on demand via `use_skill`. The bodies stay out
//! of the prompt; only one-line descriptions go in (M2.5 research notes
//! §2 — Claude Code parity).

use crate::agents::AgentRegistry;
use crate::llm::ToolSpec;
use crate::skills::SkillRegistry;

/// leek's identity — most stable section, always first.
const IDENTITY: &str = include_str!("../../../../harness/identity.md");

/// Investing-corpus orientation — how to think about the read-only
/// knowledge layer (M2.3). Static, user-maintained file under
/// `harness/`. Static include, not runtime read.
const CORPUS_ORIENTATION: &str = include_str!("../../../../harness/corpus_orientation.md");

/// A short operating note. Not methodology — just how the loop works.
const OPERATING: &str = "\
# 运行方式\n\n\
你在一个带工具的 agent loop 里运行。需要外部信息或要执行动作时调用工具，\
工具结果会回到你这里继续推理；掌握足够信息后直接作答，不要为了凑步骤而\
多调工具。回答用用户的语言。";

/// Seek-verification discipline (M2.4) — locked wording per MILESTONES
/// decision log 2026-05-20. Kept inline (not in `harness/`) because it
/// is harness behaviour, tightly coupled to the tool set (`web_search`);
/// it is not corpus content.
const SEEK_VERIFICATION_DISCIPLINE: &str = "\
# 求证纪律\n\n\
对**具体事实**（股票代码、公司、价格、新闻、事件、日期、数字等），\n\
先搜后答 —— 用 web_search 查到再回答，不要直接用训练里的世界知识。\n\
- 一次搜索返回 0 条或不相关 → 换 query 重试 1-2 次\n\
- 仍搜不到 → 明说\"搜不到\"\n\
- 若用户仍要训练知识答案 → 显式标注「以下来自训练知识，无法用搜索\n\
  证实，可能过时」再给\n\n\
分析、判断、推理是你的本职，不必为它做搜索表演。但**分析依赖的事实**\n\
必须搜过、可追溯。";

/// Build the system prompt. Sections are joined with blank lines so
/// each one reads as a distinct frame to the model.
///
/// Section order:
///   IDENTITY → OPERATING → DISCIPLINE → CORPUS_ORIENTATION
///     → # 可用工具 → # 可用 Skills → # 可用 Subagents
///
/// Subagents come after skills because they are even softer: skills are
/// recipes to import, agents are workers to spawn. The model decides
/// which to use based on whether the task wants to *learn* a procedure
/// (skill) vs *delegate* it (agent).
pub fn build_system_prompt(
    tools: &[ToolSpec],
    skills: &SkillRegistry,
    agents: &AgentRegistry,
) -> String {
    let mut p = String::with_capacity(4096);
    p.push_str(IDENTITY.trim());
    p.push_str("\n\n");
    p.push_str(OPERATING);
    p.push_str("\n\n");
    p.push_str(SEEK_VERIFICATION_DISCIPLINE);
    p.push_str("\n\n");
    p.push_str(CORPUS_ORIENTATION.trim());

    if !tools.is_empty() {
        p.push_str("\n\n# 可用工具\n\n");
        p.push_str(
            "下列工具已接入。是否调用、何时调用，由任务和每个工具自己的 \
             description（已随 API 传给你）决定 —— 这里只是一个清单。\n\n",
        );
        for t in tools {
            p.push_str(&format!(
                "- `{}` — {}\n",
                t.name,
                first_line(&t.description)
            ));
        }
    }

    let visible = skills.model_visible();
    if !visible.is_empty() {
        p.push_str("\n\n# 可用 Skills\n\n");
        p.push_str(
            "懒加载式专业上下文。需要某个专题的完整指南时，调 use_skill 工具，\
             把对应 skill 的 markdown body 加进当前 turn 的 context；之后的迭代都能看到。\n\n",
        );
        for s in visible {
            p.push_str(&s.index_line());
            p.push('\n');
        }
    }

    let agent_list = agents.iter_sorted();
    if !agent_list.is_empty() {
        p.push_str("\n\n# 可用 Subagents\n\n");
        p.push_str(
            "需要把一块**自成体系的子任务**外包出去时，用 task 工具把它交给一个 \
             subagent —— subagent 跑自己独立的 agent loop，context 跟你隔离，\
             返回 text digest。适合：corpus 跨多 doc 综合查询、可并行的 per-ticker \
             扫描、上下文压力大的深挖。不适合：一次性查个事实，那直接调对应工具更轻。\n\n",
        );
        for a in agent_list {
            p.push_str(&format!("- `{}` — {}\n", a.name, a.description.trim()));
        }
    }
    p
}

/// First sentence-ish of a description, whitespace-collapsed and capped —
/// the roster is orientation, the full schema is in the API tools array.
fn first_line(desc: &str) -> String {
    let collapsed = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = collapsed
        .find(". ")
        .map(|i| i + 1)
        .unwrap_or(collapsed.len())
        .min(160);
    let mut end = cut;
    while !collapsed.is_char_boundary(end) {
        end -= 1;
    }
    collapsed[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentLayer, AgentRegistry};
    use crate::skills::{Skill, SkillLayer, SkillRegistry};
    use std::collections::HashMap;

    fn empty_skills() -> SkillRegistry {
        SkillRegistry::default()
    }

    fn empty_agents() -> AgentRegistry {
        AgentRegistry::default()
    }

    fn one_skill_registry(name: &str, desc: &str, hidden: bool) -> SkillRegistry {
        let mut m = HashMap::new();
        m.insert(
            name.to_string(),
            Skill {
                name: name.into(),
                description: desc.into(),
                allowed_tools: vec![],
                body: "body".into(),
                source_layer: SkillLayer::Builtin,
                disable_model_invocation: hidden,
            },
        );
        SkillRegistry::new(m)
    }

    fn one_agent_registry(name: &str, desc: &str) -> AgentRegistry {
        let mut m = HashMap::new();
        m.insert(
            name.to_string(),
            AgentDef {
                name: name.into(),
                description: desc.into(),
                allowed_tools: vec![],
                system_prompt: "body".into(),
                model: None,
                cost_cap_usd: None,
                source_layer: AgentLayer::Builtin,
            },
        );
        AgentRegistry::new(m)
    }

    #[test]
    fn prompt_has_identity_and_operating() {
        let p = build_system_prompt(&[], &empty_skills(), &empty_agents());
        assert!(p.contains("leek"));
        assert!(p.contains("运行方式"));
    }

    #[test]
    fn prompt_includes_corpus_orientation() {
        let p = build_system_prompt(&[], &empty_skills(), &empty_agents());
        assert!(p.contains("corpus orientation"));
        assert!(p.contains("双轴"));
        assert!(p.contains("principles → knowledge → sources"));
    }

    #[test]
    fn prompt_has_seek_verification_discipline() {
        let p = build_system_prompt(&[], &empty_skills(), &empty_agents());
        assert!(p.contains("求证纪律"));
        assert!(p.contains("先搜后答"));
        assert!(p.contains("web_search"));
    }

    #[test]
    fn prompt_section_order_is_stable_first() {
        // IDENTITY → OPERATING → DISCIPLINE → CORPUS_ORIENTATION → tools → skills → agents.
        let tools = vec![ToolSpec {
            name: "web_fetch".into(),
            description: "Fetch a URL.".into(),
            parameters: serde_json::json!({}),
        }];
        let skills = one_skill_registry("alpha", "alpha skill", false);
        let agents = one_agent_registry("beta", "beta agent");
        let p = build_system_prompt(&tools, &skills, &agents);
        let i_identity = p.find("leek").unwrap();
        let i_operating = p.find("运行方式").unwrap();
        let i_discipline = p.find("求证纪律").unwrap();
        let i_corpus = p.find("corpus orientation").unwrap();
        let i_tools = p.find("# 可用工具").unwrap();
        let i_skills = p.find("# 可用 Skills").unwrap();
        let i_agents = p.find("# 可用 Subagents").unwrap();
        assert!(i_identity < i_operating);
        assert!(i_operating < i_discipline);
        assert!(i_discipline < i_corpus);
        assert!(i_corpus < i_tools);
        assert!(i_tools < i_skills, "skills index must come after tools");
        assert!(i_skills < i_agents, "agents index is softer than skills");
    }

    #[test]
    fn prompt_lists_tools() {
        let tools = vec![ToolSpec {
            name: "web_fetch".into(),
            description: "Fetch a URL as text. Useful for verification.".into(),
            parameters: serde_json::json!({}),
        }];
        let p = build_system_prompt(&tools, &empty_skills(), &empty_agents());
        assert!(p.contains("# 可用工具"));
        assert!(p.contains("`web_fetch`"));
    }

    #[test]
    fn prompt_lists_skills_when_registry_non_empty() {
        let p = build_system_prompt(
            &[],
            &one_skill_registry("corpus-research", "corpus research how-to", false),
            &empty_agents(),
        );
        assert!(p.contains("# 可用 Skills"));
        assert!(p.contains("`corpus-research`"));
        assert!(p.contains("corpus research how-to"));
    }

    #[test]
    fn prompt_skips_skill_section_when_only_hidden_skills() {
        let p = build_system_prompt(
            &[],
            &one_skill_registry("hidden", "hidden one", true),
            &empty_agents(),
        );
        assert!(!p.contains("# 可用 Skills"));
        assert!(!p.contains("`hidden`"));
    }

    #[test]
    fn prompt_skips_skill_section_when_registry_empty() {
        let p = build_system_prompt(&[], &empty_skills(), &empty_agents());
        assert!(!p.contains("# 可用 Skills"));
    }

    #[test]
    fn prompt_lists_agents_when_registry_non_empty() {
        let p = build_system_prompt(
            &[],
            &empty_skills(),
            &one_agent_registry("corpus-expert", "corpus 专家"),
        );
        assert!(p.contains("# 可用 Subagents"));
        assert!(p.contains("`corpus-expert`"));
        assert!(p.contains("corpus 专家"));
    }

    #[test]
    fn prompt_skips_agent_section_when_registry_empty() {
        let p = build_system_prompt(&[], &empty_skills(), &empty_agents());
        assert!(!p.contains("# 可用 Subagents"));
    }
}
