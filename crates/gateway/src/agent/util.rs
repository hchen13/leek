//! Small helpers shared across the agent loop, tools, and vendor layer.
//!
//! Everything here is `pub(crate)` because the gateway is a single binary
//! crate — there is no API to keep, and the symbols are intentionally
//! cross-module utilities (not part of the agent's external surface).

/// Truncate `s` to at most `n` chars without ever panicking on a
/// multi-byte boundary.
///
/// Returns a borrowed prefix; the caller appends an ellipsis if needed.
/// This exists because byte-indexing patterns like `&s[..n]` and
/// `s.truncate(n)` will panic mid-character when `s` carries CJK text
/// (see the panic log at `library/core/src/str/mod.rs:833`). Every
/// truncation that crosses agent-input / vendor-field boundaries goes
/// through here.
///
/// ```ignore
/// use crate::agent::util::truncate_chars;
/// assert_eq!(truncate_chars("化学制药", 2), "化学");
/// assert_eq!(truncate_chars("abc", 10), "abc");
/// ```
pub(crate) fn truncate_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_cjk_at_char_boundary() {
        assert_eq!(truncate_chars("化学制药", 2), "化学");
        assert_eq!(truncate_chars("化学制药", 0), "");
        assert_eq!(truncate_chars("化学制药", 4), "化学制药");
        assert_eq!(truncate_chars("化学制药", 10), "化学制药");
    }

    #[test]
    fn truncates_ascii_unchanged() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
        assert_eq!(truncate_chars("abcdef", 6), "abcdef");
        assert_eq!(truncate_chars("abcdef", 100), "abcdef");
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn truncates_mixed_ascii_cjk() {
        // 600276 恒瑞医药 — mixed-script prefix that previously could
        // trigger a panic when downstream code mixed byte and char
        // indexing.
        assert_eq!(truncate_chars("600276 恒瑞医药", 6), "600276");
        assert_eq!(truncate_chars("600276 恒瑞医药", 9), "600276 恒瑞");
        // Exactly at the character boundary.
        assert_eq!(truncate_chars("化学制药 (CHEM)", 5), "化学制药 ");
    }

    /// Regression: the panic in the wild reads
    /// `byte index 2 is not a char boundary; it is inside '化' (bytes
    /// 0..3) of '化学制药'`. `truncate_chars(s, n)` MUST never invoke
    /// that panic — no matter what `n` is.
    #[test]
    fn never_panics_for_any_n() {
        let s = "化学制药";
        for n in 0..=20 {
            let _ = truncate_chars(s, n);
        }
    }
}
