//! `echo` — a deterministic tool that returns its input unchanged.
//!
//! Its only job is to make the function-call loop testable: a turn that
//! calls `echo` exercises the full `function_call` → `function_call_output`
//! → continue path with a known, deterministic result.

use super::ToolOutcome;
use crate::llm::ToolSpec;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "echo".into(),
        description: "Return the given text back verbatim, character for character, \
             with no changes. A deterministic no-op. Use it to verify that tool \
             calling works end to end, or when the user explicitly asks you to echo \
             or repeat some text exactly."
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The exact text to echo back." }
            },
            "required": ["text"],
            "additionalProperties": false
        }),
    }
}

pub fn run(args: &serde_json::Value) -> ToolOutcome {
    match args.get("text").and_then(|v| v.as_str()) {
        Some(text) => ToolOutcome::ok(text.to_string()),
        None => ToolOutcome::err("echo: missing required string argument 'text'."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_text_verbatim() {
        let out = run(&serde_json::json!({ "text": "leek-m1" }));
        assert!(!out.is_error);
        assert_eq!(out.output, "leek-m1");
    }

    #[test]
    fn missing_text_is_a_structured_error() {
        let out = run(&serde_json::json!({}));
        assert!(out.is_error);
    }
}
