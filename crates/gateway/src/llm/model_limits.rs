//! Per-model context-window limits and the auto-compaction threshold derived
//! from them.
//!
//! The old code hard-coded a 380K threshold that assumed a 400K window. The
//! codex backend's `gpt-5.5` actually tops out at ~272K, so input would breach
//! the model's own limit long before 380K — auto-compaction never fired. The
//! threshold must track each model's real context size, not an absolute number.

/// Max input+output context window a model accepts, in tokens. Add a row per
/// model/provider; unknown models fall back to the conservative default.
pub fn model_max_context(model: &str) -> i64 {
    match model {
        m if m.starts_with("gpt-5") => 272_000, // codex backend gpt-5 / gpt-5.5
        _ => 272_000,
    }
}

/// Compact when the last observed input token count crosses 90% of the model's
/// window, leaving room for the next turn's user message and tool outputs.
/// `LEEK_AUTO_COMPACT_THRESHOLD` overrides the computed value (tests / tuning).
pub fn auto_compact_threshold(model: &str) -> i64 {
    std::env::var("LEEK_AUTO_COMPACT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| (model_max_context(model) as f64 * 0.90) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt5_family_window_is_272k() {
        assert_eq!(model_max_context("gpt-5.5"), 272_000);
        assert_eq!(model_max_context("gpt-5"), 272_000);
        assert_eq!(model_max_context("some-unknown-model"), 272_000);
    }

    #[test]
    fn threshold_is_ninety_percent_of_window() {
        // Ensure no env override is leaking in from the shell.
        std::env::remove_var("LEEK_AUTO_COMPACT_THRESHOLD");
        assert_eq!(auto_compact_threshold("gpt-5.5"), 244_800);
    }
}
