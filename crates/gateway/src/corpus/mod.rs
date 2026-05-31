//! Corpus → graph build pipeline.
//!
//! Spec: `design/p1-spec/corpus-brain.md`
//! Output: a static `corpus.graph.json` describing every wiki / source page
//! as a node, with wikilink + source-ref edges. Embedded into the gateway
//! binary at build time and served via `GET /api/v1/corpus/graph`.

pub mod build;
pub mod kernel;

use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct CorpusGraph {
    pub version: u32,
    pub generated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corpus_commit: Option<String>,
    pub nodes: Vec<CorpusNode>,
    pub edges: Vec<CorpusEdge>,
}

#[derive(Debug, Serialize, Clone)]
pub struct CorpusNode {
    /// Wikilink-style id, e.g. `wikis/principles/concepts/margin-of-safety`.
    pub id: String,
    /// `principles-wikis` | `principles-sources` | `knowledge-wikis` | `knowledge-sources`
    pub cluster: String,
    pub title: String,
    pub slug: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    pub tier: String,
    pub layer: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub degree: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct CorpusEdge {
    pub from: String,
    pub to: String,
    /// `wikilink` | `source-ref`
    pub kind: String,
}
