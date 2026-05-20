//! System-prompt builder — assembled most-stable-first
//! (ARCHITECTURE §4.1, best KV-cache reuse).
//!
//! Order (M2):
//!   IDENTITY  →  OPERATING  →  SEEK_VERIFICATION_DISCIPLINE  →
//!   CORPUS_ORIENTATION  →  # 可用工具
//!
//! The identity (who leek is) is fixed text. The operating note (how the
//! loop works) is short and tactical. The verification-discipline section
//! is leek's invariant: facts get searched before they get answered. The
//! corpus orientation tells the model how to think about its
//! knowledge-base — what corpus is for, how to query it. The tool roster
//! is last (it's the most volatile section, and the model also gets each
//! tool's full schema via the API tools array — so prompt prose is just
//! orientation, not the source of truth).

use crate::llm::ToolSpec;

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
pub fn build_system_prompt(tools: &[ToolSpec]) -> String {
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

    #[test]
    fn prompt_has_identity_and_operating() {
        let p = build_system_prompt(&[]);
        assert!(p.contains("leek"));
        assert!(p.contains("运行方式"));
    }

    #[test]
    fn prompt_includes_corpus_orientation() {
        // M2.3 — the orientation file's signature phrase appears verbatim
        // in the assembled prompt.
        let p = build_system_prompt(&[]);
        assert!(p.contains("corpus orientation"));
        assert!(p.contains("双轴"));
        // From the orientation doc — confirms `include_str!` actually
        // pulled the file in, not just a hard-coded blurb.
        assert!(p.contains("principles → knowledge → sources"));
    }

    #[test]
    fn prompt_has_seek_verification_discipline() {
        // M2.4 — the verification-discipline section is present with
        // its signature phrasing.
        let p = build_system_prompt(&[]);
        assert!(p.contains("求证纪律"));
        assert!(p.contains("先搜后答"));
        assert!(p.contains("web_search"));
    }

    #[test]
    fn prompt_section_order_is_stable_first() {
        // IDENTITY → OPERATING → DISCIPLINE → CORPUS_ORIENTATION → tools.
        let p = build_system_prompt(&[ToolSpec {
            name: "echo".into(),
            description: "Echo text.".into(),
            parameters: serde_json::json!({}),
        }]);
        let i_identity = p.find("leek").unwrap();
        let i_operating = p.find("运行方式").unwrap();
        let i_discipline = p.find("求证纪律").unwrap();
        let i_corpus = p.find("corpus orientation").unwrap();
        let i_tools = p.find("# 可用工具").unwrap();
        assert!(i_identity < i_operating);
        assert!(i_operating < i_discipline);
        assert!(i_discipline < i_corpus);
        assert!(i_corpus < i_tools);
    }

    #[test]
    fn prompt_lists_tools() {
        let tools = vec![ToolSpec {
            name: "echo".into(),
            description: "Echo text back. Useful for testing.".into(),
            parameters: serde_json::json!({}),
        }];
        let p = build_system_prompt(&tools);
        assert!(p.contains("# 可用工具"));
        assert!(p.contains("`echo`"));
    }
}
