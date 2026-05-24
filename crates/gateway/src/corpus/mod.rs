//! Read-only corpus — leek's investing-wisdom knowledge layer
//! (REQUIREMENTS §3, ARCHITECTURE §8). M2.1: load + lexical search.
//!
//! Files live in a separate sibling git submodule (`./corpus/`); this
//! module only LOADS them at startup and SEARCHES them in-process. The
//! corpus is read-only from the agent's perspective; promotion / editing
//! is out-of-band.
//!
//! The index is a hand-rolled BM25 (`k1 = 1.2`, `b = 0.75`) over a
//! tokenizer that splits Latin/ASCII words on whitespace and lowercases
//! them, and splits CJK per character. No stemming, no stop words, no
//! phrase queries — lexical only, on purpose (ARCHITECTURE §8 —
//! embeddings deferred).

mod bm25;
pub mod graph;
mod loader;

pub use graph::{weight_histogram, CorpusGraph};
pub use loader::{Document, Hit};

use std::path::Path;

use anyhow::Result;

use bm25::Bm25Index;
use loader::DocumentInner;

pub struct Corpus {
    docs: Vec<DocumentInner>,
    index: Bm25Index,
}

impl Corpus {
    /// Load every `*.md` under `root` into the in-memory index. A missing
    /// root yields an empty corpus + a `warn` (not an error): the agent
    /// must still serve generic answers without the knowledge layer.
    pub fn load(root: &Path) -> Result<Self> {
        if !root.exists() {
            tracing::warn!(
                root = %root.display(),
                "corpus root does not exist — running with empty corpus"
            );
            return Ok(Self::empty());
        }
        let docs = loader::walk_and_parse(root)?;
        let index = Bm25Index::build(&docs);
        Ok(Self { docs, index })
    }

    pub fn empty() -> Self {
        Self {
            docs: Vec::new(),
            index: Bm25Index::empty(),
        }
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Lexical search; returns up to `k` hits in descending BM25 order.
    /// An empty / whitespace-only query, an empty corpus, or `k == 0`
    /// returns an empty list (never errors).
    pub fn search(&self, query: &str, k: usize) -> Vec<Hit> {
        if query.trim().is_empty() || self.is_empty() || k == 0 {
            return Vec::new();
        }
        let scored = self.index.score(query, k);
        scored
            .into_iter()
            .map(|(idx, score)| {
                let doc = &self.docs[idx];
                let snippet = bm25::snippet_for(&doc.body, query, 120);
                Hit {
                    id: doc.id.clone(),
                    title: doc.title.clone(),
                    snippet,
                    score,
                }
            })
            .collect()
    }

    /// Look up a document by its `id` — the POSIX-style relative path
    /// from `corpus_root` (e.g. `sources/principles/munger/circle.md`).
    pub fn read(&self, id: &str) -> Option<Document<'_>> {
        self.docs.iter().find(|d| d.id == id).map(Document::from)
    }

    /// Build the wiki-only graph for the Corpus Brain UI (M2.1). The
    /// corpus is not hot-reloaded, so a caller usually wraps the result
    /// in `Arc` once at startup and shares it via `AppState`.
    pub fn build_graph(&self) -> CorpusGraph {
        CorpusGraph::build(&self.docs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus")
    }

    #[test]
    fn missing_corpus_dir_degrades_gracefully() {
        let bogus = PathBuf::from("/nonexistent/leek-test-path-that-does-not-exist");
        let corpus = Corpus::load(&bogus).unwrap();
        assert_eq!(corpus.len(), 0);
        assert!(corpus.is_empty());
        assert!(corpus.search("anything", 5).is_empty());
        assert!(corpus.read("any/id.md").is_none());
    }

    #[test]
    fn load_walks_and_skips_noise() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        // The fixture has: sample-en, sample-cn, no-frontmatter,
        // _meta/taxonomy. tools/* and .obsidian/* must NOT be loaded.
        assert!(corpus.len() >= 4, "expected ≥4 docs, got {}", corpus.len());
        // `_meta/` is intentionally INCLUDED (spec line 53–54).
        assert!(corpus.docs.iter().any(|d| d.id.starts_with("_meta/")));
        // `tools/` is excluded everywhere.
        assert!(
            !corpus.docs.iter().any(|d| d.id.starts_with("tools/")),
            "tools/ should be skipped, but found: {:?}",
            corpus.docs.iter().map(|d| &d.id).collect::<Vec<_>>()
        );
        // Dotfiles / dotdirs (.obsidian, .DS_Store, …) excluded.
        assert!(!corpus.docs.iter().any(|d| d.id.contains(".obsidian")));
        assert!(!corpus.docs.iter().any(|d| d.id.starts_with('.')));
    }

    #[test]
    fn id_is_posix_relative_path() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let cn = corpus
            .docs
            .iter()
            .find(|d| d.title.contains("能力圈"))
            .expect("CN fixture missing");
        assert_eq!(cn.id, "sources/principles/munger/circle.md");
        for d in &corpus.docs {
            assert!(!d.id.starts_with('/'));
            assert!(!d.id.contains('\\'));
        }
    }

    #[test]
    fn search_english_token() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let hits = corpus.search("competence", 5);
        assert!(!hits.is_empty(), "no hits for 'competence'");
        assert!(hits.iter().any(|h| h.id.contains("circle.md")));
    }

    #[test]
    fn search_cjk_char() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let hits = corpus.search("能力圈", 5);
        assert!(!hits.is_empty(), "no hits for CJK query");
        assert!(hits.iter().any(|h| h.id.contains("circle.md")));
    }

    #[test]
    fn search_misses_returns_empty() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let hits = corpus.search("zzzNotAnywhereInCorpuszzz", 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn empty_query_returns_empty() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        assert!(corpus.search("", 5).is_empty());
        assert!(corpus.search("   ", 5).is_empty());
    }

    #[test]
    fn snippet_contains_match_and_respects_utf8() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let hits = corpus.search("能力圈", 5);
        let h = &hits[0];
        // At least one of the query characters should appear in the snippet.
        assert!(h.snippet.contains('能') || h.snippet.contains('圈'));
        // Round-trip the snippet through char iteration — a broken UTF-8
        // boundary would panic here.
        let _: Vec<char> = h.snippet.chars().collect();
    }

    #[test]
    fn frontmatter_parsed_or_empty_on_garbage() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let en = corpus
            .docs
            .iter()
            .find(|d| d.id == "sources/knowledge/example.md")
            .expect("EN fixture missing");
        assert_eq!(
            en.frontmatter.get("title").map(|s| s.as_str()),
            Some("Sample English")
        );
        let bare = corpus
            .docs
            .iter()
            .find(|d| d.id == "no-frontmatter.md")
            .expect("bare fixture missing");
        assert!(bare.frontmatter.is_empty());
        assert_eq!(bare.title, "Bare File");
    }

    #[test]
    fn read_returns_doc_by_id() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let doc = corpus
            .read("sources/principles/munger/circle.md")
            .expect("circle.md missing");
        // Exercise every Document field so the M2.2 tools' contract is
        // covered before they exist.
        assert_eq!(doc.id, "sources/principles/munger/circle.md");
        assert!(doc.title.contains("能力圈"));
        assert_eq!(
            doc.frontmatter.get("tier").map(|s| s.as_str()),
            Some("principles")
        );
        assert!(doc.body.contains("能力圈") || doc.body.contains("circle"));
        assert!(corpus.read("does/not/exist.md").is_none());
    }

    #[test]
    fn hit_carries_title_and_score() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        let hits = corpus.search("能力圈", 5);
        let h = &hits[0];
        // Score should be a positive real (BM25 idf is positive for terms
        // present in fewer than half the docs).
        assert!(h.score > 0.0, "expected positive score, got {}", h.score);
        assert!(h.title.contains("能力圈"));
    }

    #[test]
    fn limit_truncates_results() {
        let corpus = Corpus::load(&fixtures()).unwrap();
        // "the" appears in many fixture docs; limit to 1.
        let hits = corpus.search("the", 1);
        assert!(hits.len() <= 1);
    }

    /// PM acceptance check §3 — sanity-search against the *real* corpus
    /// at the workspace root. `#[ignore]` because it depends on the
    /// sibling submodule being present; run it with
    /// `cargo test -p leek-gateway -- --ignored --nocapture
    /// corpus_sanity_real_corpus`.
    #[test]
    #[ignore = "needs real ./corpus/ at workspace root"]
    fn corpus_sanity_real_corpus() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // `crates/gateway/` → workspace root → `./corpus`.
        let root = manifest
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("corpus");
        let corpus = Corpus::load(&root).unwrap();
        eprintln!("corpus size: {}", corpus.len());
        assert!(
            corpus.len() > 100,
            "expected a populated corpus, got {}",
            corpus.len()
        );
        let hits = corpus.search("munger circle of competence", 5);
        eprintln!("\ntop 5 hits for 'munger circle of competence':");
        for (i, h) in hits.iter().enumerate() {
            eprintln!("  {}. [{:.3}] {} — {}", i + 1, h.score, h.id, h.title);
        }
        assert!(!hits.is_empty(), "no hits");
        assert!(
            hits.iter().any(|h| h.id.contains("munger")),
            "expected a hit whose id contains 'munger', got: {:?}",
            hits.iter().map(|h| &h.id).collect::<Vec<_>>()
        );
    }
}
