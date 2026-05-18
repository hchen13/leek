//! Per-model token pricing + context-window sizes.
//!
//! Used by the cost cap (M1) and `turn_metrics` cost reporting. Prices are
//! USD per 1,000,000 tokens. Lookup is exact-match by model name; an unknown
//! model returns `None` / `0.0` cost — a safer default than guessing a wrong
//! price and tripping the cost cap falsely.
//!
//! Prices are baked in, not config-driven: cost is plumbing, not a user
//! preference, and a misconfigured price is a worse failure than a stale one.
//! The `gpt-5.5` row is a leek-side estimate — the codex backend does not
//! publish a pricing surface. Revise when vendor numbers change.

use super::Usage;

/// Per-model token pricing (USD per 1M tokens) plus context window size.
#[derive(Debug, Clone, Copy)]
pub struct ModelPrice {
    pub input_per_million: f64,
    pub output_per_million: f64,
    /// Cache-hit input rate. `None` → use `input_per_million` for cache hits.
    pub cached_input_per_million: Option<f64>,
    pub context_window_tokens: i64,
}

/// Effective context window for `model`, in tokens. Defaults to 400K when
/// unknown — conservative on the high side so the auto-compaction trigger
/// does not over-fire on a model we simply do not have a row for.
pub fn context_window(model: &str) -> i64 {
    lookup(model)
        .map(|p| p.context_window_tokens)
        .unwrap_or(400_000)
}

/// Exact-match price lookup (case / whitespace insensitive). `None` for an
/// unknown model — callers treat that as "cost unknown".
pub fn lookup(model: &str) -> Option<ModelPrice> {
    match model.trim().to_ascii_lowercase().as_str() {
        // gpt-5.5 — leek's model via the codex backend. Pricing is an estimate.
        "gpt-5.5" => Some(ModelPrice {
            input_per_million: 5.0,
            output_per_million: 15.0,
            cached_input_per_million: Some(0.5),
            context_window_tokens: 400_000,
        }),
        "gpt-5" => Some(ModelPrice {
            input_per_million: 1.25,
            output_per_million: 10.0,
            cached_input_per_million: Some(0.125),
            context_window_tokens: 400_000,
        }),
        "gpt-5-mini" => Some(ModelPrice {
            input_per_million: 0.25,
            output_per_million: 2.0,
            cached_input_per_million: Some(0.025),
            context_window_tokens: 200_000,
        }),
        _ => None,
    }
}

/// USD cost of one `Usage` block at `model`'s pricing. Returns `0.0` for an
/// unknown model — the caller treats that as "cost cap disabled for this call".
pub fn compute_cost(model: &str, usage: &Usage) -> f64 {
    let Some(price) = lookup(model) else {
        return 0.0;
    };
    let cached = usage.cache_read_tokens as f64;
    // `input_tokens` is the *total* input (cache hits included); the
    // non-cached portion is the remainder. Saturating subtract guards
    // against a vendor reporting cached > total.
    let non_cached = (usage.input_tokens as f64 - cached).max(0.0);
    let cached_rate = price
        .cached_input_per_million
        .unwrap_or(price.input_per_million);
    (non_cached * price.input_per_million
        + cached * cached_rate
        + usage.output_tokens as f64 * price.output_per_million)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_and_whitespace_insensitive() {
        assert!(lookup("  GPT-5.5 ").is_some());
        assert!(lookup("not-a-model").is_none());
    }

    #[test]
    fn context_window_falls_back_for_unknown() {
        assert_eq!(context_window("gpt-5.5"), 400_000);
        assert_eq!(context_window("gpt-5-mini"), 200_000);
        assert_eq!(context_window("not-a-model"), 400_000);
    }

    #[test]
    fn compute_cost_unknown_model_is_zero() {
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
        };
        assert_eq!(compute_cost("not-a-model", &u), 0.0);
    }

    #[test]
    fn compute_cost_splits_cached_input() {
        // gpt-5: input 1.25, cached 0.125, output 10.0 (per 1M).
        // 10_000 input of which 8_000 cached → 2_000 non-cached.
        //   2_000*1.25 + 8_000*0.125 + 1_000*10.0 = 2_500 + 1_000 + 10_000
        //   = 13_500 / 1e6 = 0.0135
        let u = Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cache_read_tokens: 8_000,
        };
        assert!((compute_cost("gpt-5", &u) - 0.0135).abs() < 1e-9);
    }

    #[test]
    fn compute_cost_never_negative() {
        let u = Usage {
            input_tokens: 100,
            output_tokens: 0,
            cache_read_tokens: 200, // over-reported; must not go negative
        };
        assert!(compute_cost("gpt-5", &u) >= 0.0);
    }
}
