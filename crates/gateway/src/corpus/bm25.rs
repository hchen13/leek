//! Hand-rolled BM25 + tokenizer (~150 lines). Hand-rolled on purpose:
//! corpus search is part of the project's identity and pinning behaviour
//! across milestones is easier without coupling to a third-party crate's
//! choices (ARCHITECTURE §8 — lexical only, embeddings deferred).
//!
//! Formula: standard BM25 with k1 = 1.2, b = 0.75.
//! Tokenization: per-character on CJK; whitespace + lowercase on
//! Latin / ASCII words. No stemming, no stop words, no phrase queries.

use std::collections::HashMap;

use super::loader::DocumentInner;

const K1: f32 = 1.2;
const B: f32 = 0.75;

pub struct Bm25Index {
    /// token → doc_idx → term frequency in that doc.
    postings: HashMap<String, HashMap<usize, u32>>,
    /// doc_idx → tokenized length.
    doc_lens: Vec<u32>,
    avg_dl: f32,
    n_docs: usize,
}

impl Bm25Index {
    pub fn empty() -> Self {
        Self {
            postings: HashMap::new(),
            doc_lens: Vec::new(),
            avg_dl: 0.0,
            n_docs: 0,
        }
    }

    /// Build the inverted index from the loader's documents. Title and
    /// body are tokenized together with equal weight — the title is short
    /// (~5 tokens) and the body is hundreds, so title boost is a
    /// rounding error; keep the algorithm simple.
    pub fn build(docs: &[DocumentInner]) -> Self {
        if docs.is_empty() {
            return Self::empty();
        }
        let mut postings: HashMap<String, HashMap<usize, u32>> = HashMap::new();
        let mut doc_lens: Vec<u32> = Vec::with_capacity(docs.len());
        for (idx, d) in docs.iter().enumerate() {
            let toks = tokens_of(&d.title, &d.body);
            doc_lens.push(toks.len() as u32);
            for t in toks {
                *postings.entry(t).or_default().entry(idx).or_insert(0) += 1;
            }
        }
        let total: u64 = doc_lens.iter().copied().map(u64::from).sum();
        let avg_dl = if doc_lens.is_empty() {
            0.0
        } else {
            total as f32 / doc_lens.len() as f32
        };
        Self {
            postings,
            doc_lens,
            avg_dl,
            n_docs: docs.len(),
        }
    }

    /// Score docs matching at least one query token; return top `k`
    /// `(idx, score)` in descending score, then ascending idx for
    /// determinism on ties.
    pub fn score(&self, query: &str, k: usize) -> Vec<(usize, f32)> {
        let q_toks = tokenize(query);
        if q_toks.is_empty() || self.n_docs == 0 {
            return Vec::new();
        }
        let mut scores: HashMap<usize, f32> = HashMap::new();
        for q in &q_toks {
            let Some(pl) = self.postings.get(q) else {
                continue;
            };
            let df = pl.len() as f32;
            // BM25 idf with +0.5 / +0.5 stabilizer and +1 to keep ln positive.
            let idf = ((self.n_docs as f32 - df + 0.5) / (df + 0.5) + 1.0).ln();
            for (&doc_idx, &tf) in pl {
                let dl = self.doc_lens[doc_idx] as f32;
                let denom = tf as f32 + K1 * (1.0 - B + B * dl / self.avg_dl.max(1.0));
                let term_score = idf * (tf as f32 * (K1 + 1.0)) / denom;
                *scores.entry(doc_idx).or_insert(0.0) += term_score;
            }
        }
        let mut ranked: Vec<(usize, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        ranked.truncate(k);
        ranked
    }
}

/// Tokenize a chunk of text into search tokens. Same algorithm everywhere
/// — building the index and scoring queries — so a token that goes in is
/// the same token that gets looked up.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        if is_cjk(ch) {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf).to_lowercase());
            }
            out.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            buf.push(ch);
        } else {
            // Whitespace, punctuation, anything else — boundary.
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf).to_lowercase());
            }
        }
    }
    if !buf.is_empty() {
        out.push(buf.to_lowercase());
    }
    out
}

fn tokens_of(title: &str, body: &str) -> Vec<String> {
    let mut t = tokenize(title);
    t.extend(tokenize(body));
    t
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x309F |  // Hiragana
        0x30A0..=0x30FF |  // Katakana
        0x3400..=0x4DBF |  // CJK Extension A
        0x4E00..=0x9FFF |  // CJK Unified
        0xAC00..=0xD7AF    // Hangul syllables
    )
}

/// `±width` byte window around the first matching token in `body`,
/// clamped to UTF-8 character boundaries. Falls back to the body's start
/// if no query token appears (shouldn't happen for a body that scored,
/// but the snippet is best-effort).
pub fn snippet_for(body: &str, query: &str, width: usize) -> String {
    let toks = tokenize(query);
    let lower_body = body.to_lowercase();
    let mut earliest: Option<usize> = None;
    for t in &toks {
        if let Some(pos) = lower_body.find(t.as_str()) {
            earliest = match earliest {
                Some(e) if e <= pos => Some(e),
                _ => Some(pos),
            };
        }
    }
    let center = earliest.unwrap_or(0);
    let start = floor_char_boundary(body, center.saturating_sub(width));
    let end = ceil_char_boundary(body, (center + width).min(body.len()));
    let slice = body[start..end].trim();
    let mut out = String::with_capacity(slice.len() + 6);
    if start > 0 {
        out.push('…');
    }
    out.push_str(slice);
    if end < body.len() {
        out.push('…');
    }
    out
}

fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_cjk_per_char() {
        let t = tokenize("能力圈");
        assert_eq!(t, vec!["能", "力", "圈"]);
    }

    #[test]
    fn tokenize_lowercases_latin() {
        let t = tokenize("Hello World");
        assert_eq!(t, vec!["hello", "world"]);
    }

    #[test]
    fn tokenize_mixed_cjk_and_latin() {
        let t = tokenize("Apple 公司 NVDA 1");
        assert_eq!(t, vec!["apple", "公", "司", "nvda", "1"]);
    }

    #[test]
    fn tokenize_ignores_punctuation() {
        let t = tokenize("hello, world! — 一段中文。");
        assert_eq!(t, vec!["hello", "world", "一", "段", "中", "文"]);
    }

    #[test]
    fn empty_query_scores_nothing() {
        let idx = Bm25Index::empty();
        assert!(idx.score("anything", 5).is_empty());
    }

    #[test]
    fn snippet_clamps_to_char_boundary() {
        // CJK chars are 3 bytes each — naive byte slicing would split them.
        let body = "短文：能力圈是 Munger 的关键概念之一。它说的是只在你真正懂的领域里下注。";
        let s = snippet_for(body, "能力圈", 20);
        assert!(s.contains("能"));
        // Iterating chars on the snippet would panic if a boundary were broken.
        let _: Vec<char> = s.chars().collect();
    }

    #[test]
    fn snippet_no_match_returns_prefix() {
        let body = "这是一段没有匹配的文本。";
        let s = snippet_for(body, "完全不出现的词", 20);
        // Still returns something — best-effort — and is valid UTF-8.
        let _: Vec<char> = s.chars().collect();
    }
}
