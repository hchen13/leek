//! System-prompt builder — deliberately minimal for M1.
//!
//! ARCHITECTURE §4.1 orders the system prompt most-stable-first. M1 only has
//! the parts that exist yet: the identity, and the tool roster. Corpus
//! orientation (M2) and the skill index (M2.5) slot in later — they are NOT
//! introduced here. No tool-usage prose: the model picks tools from the
//! task and each tool's own description (sent via the API tools array).

use crate::llm::ToolSpec;

/// leek's identity — most stable section, always first (best KV-cache reuse).
const IDENTITY: &str = include_str!("../../../../harness/identity.md");

/// A short operating note. Not methodology — just how the loop works.
const OPERATING: &str = "\
# 运行方式\n\n\
你在一个带工具的 agent loop 里运行。需要外部信息或要执行动作时调用工具，\
工具结果会回到你这里继续推理；掌握足够信息后直接作答，不要为了凑步骤而\
多调工具。回答用用户的语言。";

/// Build the M1 system prompt: identity + operating note + tool roster.
pub fn build_system_prompt(tools: &[ToolSpec]) -> String {
    let mut p = String::with_capacity(2048);
    p.push_str(IDENTITY.trim());
    p.push_str("\n\n");
    p.push_str(OPERATING);

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

/// First sentence-ish of a description, whitespace-collapsed and capped — the
/// roster is orientation, the full schema is in the API tools array.
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
    fn prompt_has_identity_and_no_corpus() {
        let p = build_system_prompt(&[]);
        assert!(p.contains("leek"));
        assert!(p.contains("运行方式"));
        // M1 must not pull in the corpus-orientation section.
        assert!(!p.contains("corpus orientation"));
        assert!(!p.contains("# 可用工具"));
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
