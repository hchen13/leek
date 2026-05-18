//! Client-side function tools — the M1 tool set.
//!
//! M1 keeps the surface tiny: one deterministic tool (`echo`) to prove the
//! function-call loop, and one real tool (`web_fetch`). The domain tools
//! (quotes, financials, …) belong to M3. Tool names and descriptions are
//! vendor-neutral (ARCHITECTURE §12.1).

mod echo;
mod web_fetch;

use crate::llm::ToolSpec;

/// Outcome of running one tool call. `is_error` lets the agent loop count
/// tool errors and lets the model see, in the `function_call_output`, that a
/// call failed (so it can recover) — without the failure aborting the turn.
pub struct ToolOutcome {
    pub output: String,
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    fn err(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

/// The function-tool specs offered to the model this milestone.
pub fn specs() -> Vec<ToolSpec> {
    vec![echo::spec(), web_fetch::spec()]
}

/// Dispatch one function call. An unknown name is a structured error, not a
/// panic — the model gets it back as `function_call_output` and can recover.
pub async fn dispatch(http: &reqwest::Client, name: &str, args: &serde_json::Value) -> ToolOutcome {
    match name {
        "echo" => echo::run(args),
        "web_fetch" => web_fetch::run(http, args).await,
        other => ToolOutcome::err(format!(
            "unknown tool '{other}' — it is not in the available tool set"
        )),
    }
}
