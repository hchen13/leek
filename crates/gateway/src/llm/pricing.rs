//! Per-model token pricing — used by M1.5 cost cap and `task_metrics`
//! reporting.
//!
//! Prices are stored as USD per 1,000,000 tokens, following the OpenAI
//! / Anthropic / etc. public pricing convention. Lookups are exact-
//! match by model name (`ChatRequest.model`, e.g. `"gpt-5.5"`); when
//! a model isn't in the table `lookup` returns `None` and
//! `compute_cost` returns `0.0` — that effectively disables the cost
//! cap for unknown models, which is the safer default than guessing
//! a wrong price and falsely tripping the guard.
//!
//! When prices change (vendor updates), edit this file and ship a new
//! release. The table is intentionally not file-or-env-driven — cost
//! is plumbing, not user preference, and a misconfigured price would
//! be a worse failure mode than a stale-but-bounded one.
//!
//! Cache pricing: when a model has a separate "cached input" rate
//! (OpenAI's prompt caching, Anthropic's prompt caching), we apply
//! that rate to the `cache_read_tokens` portion of the Usage block.
//! Models without a separate cached rate use the regular input rate
//! for cache reads (a conservative overestimate when reading from a
//! cache that was actually free).

use super::Usage;

/// Per-model token pricing in USD per 1M tokens.
#[derive(Debug, Clone, Copy)]
pub struct ModelPrice {
    pub input_per_million: f64,
    pub output_per_million: f64,
    /// Cached input rate (cache *hit* tokens). `None` means the model
    /// has no separate cache pricing — use the regular `input_per_million`
    /// rate for cache hits.
    pub cached_input_per_million: Option<f64>,
    /// Effective context window in tokens. Used by the auto-compactor
    /// to derive the absolute trigger from `tuning.guards.auto_compact_threshold`
    /// (a fraction). When a model isn't in the table, callers should
    /// fall back to a conservative default (see `MAIN_CONTEXT_WINDOW_TOKENS`
    /// in api/messages.rs) — guessing wrong here means we either
    /// over-trigger compaction (cheap) or under-trigger and overflow
    /// the window (catastrophic).
    pub context_window_tokens: i64,
}

/// Convenience accessor: `pricing::context_window(model)` returns the
/// model's effective window in tokens. Defaults to 400K (gpt-5.5
/// equivalent) when unknown — conservative on the high side.
///
/// R3 fix: `api::messages::auto_compact_threshold` previously hardcoded
/// 400K regardless of which model the active call uses. Once future
/// surfaces (skills, subagents) start specifying per-call models, that
/// hardcode silently fails on smaller-window models. This lookup gives
/// the right number per model without a separate table.
pub fn context_window(model: &str) -> i64 {
    lookup(model).map(|p| p.context_window_tokens).unwrap_or(400_000)
}

/// Exact-match lookup. Returns `None` for unknown models — callers
/// should treat that as "cost unknown" and skip cost-based guards.
///
/// Prices below are approximations sourced from public vendor docs as
/// of 2026-05; the gpt-5.5 row in particular is a leek-side estimate
/// since the codex backend's pricing surface isn't part of the public
/// API. Update with vendor announcements.
pub fn lookup(model: &str) -> Option<ModelPrice> {
    let m = model.trim().to_ascii_lowercase();
    match m.as_str() {
        // OpenAI gpt-5 family — public pricing
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
        // gpt-5.5 — leek's default model via codex backend. Pricing is
        // an estimate; revise when public numbers exist.
        "gpt-5.5" => Some(ModelPrice {
            input_per_million: 5.0,
            output_per_million: 15.0,
            cached_input_per_million: Some(0.5),
            context_window_tokens: 400_000,
        }),
        // Common helper-tier models in case other surfaces ever request
        // them directly.
        "gpt-4.1" | "gpt-4o" => Some(ModelPrice {
            input_per_million: 2.5,
            output_per_million: 10.0,
            cached_input_per_million: Some(1.25),
            context_window_tokens: 128_000,
        }),
        "gpt-4o-mini" => Some(ModelPrice {
            input_per_million: 0.15,
            output_per_million: 0.6,
            cached_input_per_million: Some(0.075),
            context_window_tokens: 128_000,
        }),
        // o-series (reasoning)
        "o1" | "o1-2024-12-17" => Some(ModelPrice {
            input_per_million: 15.0,
            output_per_million: 60.0,
            cached_input_per_million: Some(7.5),
            context_window_tokens: 200_000,
        }),
        "o1-mini" => Some(ModelPrice {
            input_per_million: 3.0,
            output_per_million: 12.0,
            cached_input_per_million: Some(1.5),
            context_window_tokens: 128_000,
        }),
        "o3" => Some(ModelPrice {
            input_per_million: 10.0,
            output_per_million: 40.0,
            cached_input_per_million: Some(2.5),
            context_window_tokens: 200_000,
        }),
        "o3-mini" | "o4-mini" => Some(ModelPrice {
            input_per_million: 1.1,
            output_per_million: 4.4,
            cached_input_per_million: Some(0.275),
            context_window_tokens: 200_000,
        }),
        _ => None,
    }
}

/// USD cost of a single LLM `Usage` block at the given model's
/// pricing. Returns `0.0` for unknown models (caller treats as
/// "cost-cap disabled for this call").
pub fn compute_cost(model: &str, usage: &Usage) -> f64 {
    let Some(price) = lookup(model) else {
        return 0.0;
    };
    let cached = usage.cache_read_tokens as f64;
    // `input_tokens` reported by the upstream is the *total* input
    // count (including cache hits), so the non-cached portion is the
    // remainder. Saturating subtract guards against any vendor
    // reporting cached > total (shouldn't happen but cheap insurance).
    let total_input = usage.input_tokens as f64;
    let non_cached = (total_input - cached).max(0.0);
    let output = usage.output_tokens as f64;
    let cached_rate = price
        .cached_input_per_million
        .unwrap_or(price.input_per_million);
    (non_cached * price.input_per_million
        + cached * cached_rate
        + output * price.output_per_million)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_handles_case_and_whitespace() {
        assert!(lookup("GPT-5").is_some());
        assert!(lookup("  gpt-5  ").is_some());
    }

    #[test]
    fn lookup_unknown_model_is_none() {
        assert!(lookup("not-a-model").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn compute_cost_unknown_model_returns_zero() {
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        assert_eq!(compute_cost("not-a-model", &u), 0.0);
    }

    #[test]
    fn compute_cost_simple_input_output() {
        // gpt-5: $1.25/M input, $10/M output. 1000 in + 500 out:
        //   1000 * 1.25 / 1e6  + 500 * 10 / 1e6 = 0.00125 + 0.005 = 0.00625
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let c = compute_cost("gpt-5", &u);
        assert!((c - 0.00625).abs() < 1e-9, "expected ~0.00625, got {c}");
    }

    #[test]
    fn compute_cost_splits_cached_from_normal_input() {
        // gpt-5: input $1.25/M, cached input $0.125/M, output $10/M.
        // 10_000 total input of which 8_000 are cache reads:
        //   non_cached = 2_000 → 2_000 * 1.25 / 1e6 = 0.0025
        //   cached     = 8_000 → 8_000 * 0.125 / 1e6 = 0.001
        //   output     = 1_000 → 1_000 * 10.0 / 1e6 = 0.01
        // total = 0.0135
        let u = Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            cache_read_tokens: 8_000,
            cache_write_tokens: 0,
        };
        let c = compute_cost("gpt-5", &u);
        assert!((c - 0.0135).abs() < 1e-9, "expected ~0.0135, got {c}");
    }

    #[test]
    fn compute_cost_handles_cached_exceeds_total_gracefully() {
        // Vendor over-reports cache reads vs total — saturate to 0.
        let u = Usage {
            input_tokens: 100,
            output_tokens: 0,
            cache_read_tokens: 200, // > total; should not produce negative cost
            cache_write_tokens: 0,
        };
        let c = compute_cost("gpt-5", &u);
        // non_cached = max(100 - 200, 0) = 0; cached = 200; output = 0
        // Just cached: 200 * 0.125 / 1e6 = 0.000025
        assert!(c >= 0.0, "cost must not go negative");
        assert!((c - 0.000025).abs() < 1e-12);
    }
}
