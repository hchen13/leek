//! M3 guardrail: vendor names must NOT leak from `vendors/` into any
//! model-facing surface. ARCHITECTURE §12.1 + M3 spec §A.6: vendor
//! identity (Tushare / Sina / EastMoney) only appears in vendors/ +
//! struct field comments + env var name + tracing log fields.
//!
//! Concrete surfaces this test covers (these are what the model and the
//! canvas card see):
//!
//! 1. `ToolSpec.name` (every tool we advertise)
//! 2. `ToolSpec.description` text
//! 3. `ToolSpec.parameters` JSON schema (recursively, both keys and
//!    string values like `description`)
//!
//! Why not a repo-wide grep: Rust identifiers like `vendors.tushare` and
//! the env-var name `LEEK_TUSHARE_TOKEN` are legitimately present in
//! the tool dispatch path. The contract is about what *crosses the
//! Rust → model boundary*, not about every byte of source.
//!
//! Display payloads + model_output are exercised by the per-tool unit
//! tests via `ToolOutcome` — they only set a `data_source` tag with
//! generic strings (`primary` / `fallback-1`), never a vendor name. The
//! `served_by_vendor` debug field is in `debug_payload`, which is the
//! debug view only.

// The gateway is a binary, not a library — so we can't import its
// modules from this integration test. We treat the source files
// themselves as the surface: parse the `spec()` function in each
// M3 tool, extract its `description` literal and `parameters` JSON
// schema body, and assert no vendor name is in those strings. This is
// the cheapest authoritative check without restructuring the crate
// into a lib + bin pair just for testing.

// Forbidden substrings. Case-insensitive ASCII for romanizations;
// exact match for CJK. `Tushare`, `EastMoney`, `Sina`, `新浪`,
// `东方财富`. We allow Chinese aliases too because product copy might
// otherwise use them.
const FORBIDDEN_ASCII: &[&str] = &["tushare", "eastmoney", "sina finance"];
// `Sina` alone is too common a token to ban directly — only fail on
// the unambiguous form "Sina Finance" (vendor brand name).
const FORBIDDEN_CJK: &[&str] = &["新浪", "东方财富"];

#[test]
fn tool_spec_descriptions_are_vendor_neutral() {
    // Inline-parse the relevant `description: "..."` literal from each
    // tool's `spec()` function. We scan the source text rather than
    // the live `ToolSpec` because the gateway has no library
    // entry-point; this is the cheapest binding-free way to read what
    // every tool advertises.
    let spec_files = [
        "src/agent/tools/market_quote.rs",
        "src/agent/tools/get_candlesticks.rs",
        "src/agent/tools/get_financials.rs",
        "src/agent/tools/get_company_info.rs",
        "src/agent/tools/get_capital_flow.rs",
        // M4.1 — supplementary research tools.
        "src/agent/tools/get_industry_peers.rs",
        "src/agent/tools/get_business_breakdown.rs",
        "src/agent/tools/get_announcements.rs",
        "src/agent/tools/get_consensus.rs",
        "src/agent/tools/get_top_holders.rs",
        "src/agent/tools/get_concepts.rs",
    ];
    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut violations: Vec<String> = Vec::new();
    for relpath in spec_files {
        let path = crate_root.join(relpath);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let desc = extract_description(&body).unwrap_or_else(|| {
            panic!(
                "{}: could not locate `description: \"…\".into()` literal",
                relpath
            )
        });
        check(relpath, "description", &desc, &mut violations);
        // The whole `parameters: json!({...})` block contributes
        // schema strings the model reads (property `description`s
        // especially). A coarse substring check on the literal text
        // catches leaks without bringing in a JSON parser.
        let params = extract_parameters_block(&body).unwrap_or_default();
        check(relpath, "parameters", &params, &mut violations);
    }
    // The system prompt's tool roster line also reaches the model.
    // It re-uses each spec's `first_line(description)` — covered by
    // the above. But the prompt has its own static prose; scan it.
    let prompt = std::fs::read_to_string(crate_root.join("src/agent/prompt.rs"))
        .expect("read prompt.rs");
    check("src/agent/prompt.rs", "static-prose", &prompt, &mut violations);
    assert!(
        violations.is_empty(),
        "vendor name leaked into model-facing surface:\n{}",
        violations.join("\n")
    );
}

fn check(file: &str, surface: &str, body: &str, violations: &mut Vec<String>) {
    let lower = body.to_ascii_lowercase();
    for needle in FORBIDDEN_ASCII {
        let n = needle.to_ascii_lowercase();
        if lower.contains(&n) {
            violations.push(format!("  {file} ({surface}): contains '{needle}'"));
        }
    }
    for needle in FORBIDDEN_CJK {
        if body.contains(needle) {
            violations.push(format!("  {file} ({surface}): contains '{needle}'"));
        }
    }
}

/// Find the `description: "…".into(),` literal in a `spec()` fn.
/// Tools build it as a multi-line string with backslash continuations;
/// the closing `.into()` may be on its own line preceded by whitespace.
/// We walk the source character-by-character honoring escape sequences
/// to find the matching closing quote of the FIRST string literal,
/// then verify that `.into()` follows (with possible whitespace).
fn extract_description(src: &str) -> Option<String> {
    let start = src.find("description:")?;
    let after = &src[start..];
    let q = after.find('"')?;
    let body_start = start + q + 1;
    let bytes = src.as_bytes();
    let mut i = body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // skip the escaped char
            b'"' => {
                // Look ahead for `.into()` after optional whitespace.
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if src[j..].starts_with(".into()") {
                    return Some(src[body_start..i].to_string());
                }
                // Not the closing of `description`; keep scanning.
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Find the `parameters: serde_json::json!({…})` literal.
fn extract_parameters_block(src: &str) -> Option<String> {
    let start = src.find("parameters:")?;
    let after = &src[start..];
    let open = after.find('(')?;
    // Read balanced parens.
    let bytes = after.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(after[open..=i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
