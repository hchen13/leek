//! Wiki graph — nodes for every `wikis/**.md` doc, edges over shared tags.
//!
//! M2.1 Corpus Brain UI data source. The full graph is the **wiki layer**
//! only (REQUIREMENTS §2.5 — Corpus Brain visualises curated knowledge,
//! not raw sources). Nodes carry the slice of frontmatter the UI needs to
//! render (tier / type / tags / confidence / title); edges are derived
//! from shared tags, weighted by overlap, and pruned at `weight ≥ 2` so
//! the graph stays readable (~hundreds of edges, not thousands).
//!
//! Built once at startup over the loaded `Corpus`; the agent never edits
//! the corpus in-process, so a cached `Arc<CorpusGraph>` on `AppState`
//! is the only consumer.

use std::collections::HashMap;

use serde::Serialize;

use super::loader::DocumentInner;

/// A node in the wiki graph — one `wikis/**.md` document.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    /// The document id (POSIX path from corpus root). Matches the id
    /// returned by `corpus_search` / `corpus_read` so the frontend can
    /// pivot from a tool event straight onto a node.
    pub id: String,
    /// Frontmatter `title` → first `#` heading → file stem (loader's
    /// precedence). Always non-empty for a well-formed wiki.
    pub title: String,
    /// `principles` | `knowledge` — derived from frontmatter `tier` when
    /// present, otherwise from the path segment under `wikis/`.
    pub tier: String,
    /// `concept` | `entity` | `topic` | `comparison` | … — frontmatter
    /// `type`, or empty if absent.
    #[serde(rename = "type")]
    pub doc_type: String,
    /// Frontmatter `tags`, parsed from the comma-joined string the YAML
    /// loader stores (empty when absent).
    pub tags: Vec<String>,
    /// Frontmatter `confidence` (`high` | `medium` | `low`) — drives node
    /// size in the brain UI; empty if absent.
    pub confidence: String,
    /// Frontmatter `slug` (the canonical short name) — empty if absent.
    pub slug: String,
    /// Directory of the wiki (e.g. `wikis/principles/concepts`) — used by
    /// the frontend for clustering / hover context.
    pub directory_path: String,
}

/// An edge in the wiki graph — derived from shared frontmatter tags.
/// `weight` is the count of overlapping tags between the two endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub weight: u32,
    pub shared_tags: Vec<String>,
}

/// The full wiki graph — `nodes` is sorted by id (loader order) and
/// `edges` is sorted by `(source, target)` for deterministic JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct CorpusGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Minimum shared-tag overlap to keep an edge. With `MIN_EDGE_WEIGHT = 2`
/// the graph stays readable on 100-200 nodes — a single shared tag like
/// "buffett" would otherwise saturate the canvas.
const MIN_EDGE_WEIGHT: u32 = 2;

impl CorpusGraph {
    /// Build the graph from the loaded corpus. Only `wikis/**.md` docs
    /// contribute nodes — `sources/`, `_meta/`, and anything outside
    /// `wikis/` is skipped (REQUIREMENTS §2.5).
    pub fn build(docs: &[DocumentInner]) -> Self {
        let mut nodes: Vec<GraphNode> = docs
            .iter()
            .filter(|d| is_wiki_doc(&d.id))
            .map(node_from_doc)
            .collect();
        // Already in id-ascending order (loader sorts), but guarantee it
        // here so a future loader change doesn't quietly break the API
        // contract.
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let edges = build_edges(&nodes);
        Self { nodes, edges }
    }
}

/// `wikis/...` or `wikis\...`? The loader normalises to POSIX, so we only
/// have to check the forward-slash form.
fn is_wiki_doc(id: &str) -> bool {
    id == "wikis" || id.starts_with("wikis/")
}

fn node_from_doc(d: &DocumentInner) -> GraphNode {
    let tier = d
        .frontmatter
        .get("tier")
        .cloned()
        .unwrap_or_else(|| tier_from_path(&d.id));
    let doc_type = d.frontmatter.get("type").cloned().unwrap_or_default();
    let confidence = d
        .frontmatter
        .get("confidence")
        .cloned()
        .unwrap_or_default();
    let slug = d.frontmatter.get("slug").cloned().unwrap_or_default();
    let tags = parse_tags(d.frontmatter.get("tags").map(String::as_str).unwrap_or(""));
    let directory_path = directory_of(&d.id);

    GraphNode {
        id: d.id.clone(),
        title: d.title.clone(),
        tier,
        doc_type,
        tags,
        confidence,
        slug,
        directory_path,
    }
}

/// Infer tier from a path like `wikis/principles/concepts/foo.md`.
/// Returns an empty string when the path is malformed (shouldn't happen
/// for valid wiki docs, but we won't panic on a weird layout).
fn tier_from_path(id: &str) -> String {
    id.strip_prefix("wikis/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(tier, _)| tier.to_string())
        .unwrap_or_default()
}

fn directory_of(id: &str) -> String {
    id.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// Parse the loader's comma-joined tag string (`"buffett, munger, …"`)
/// into a deduplicated, lowercase, order-preserving list. The loader
/// strips the surrounding `[]` of a flat YAML array before this point.
fn parse_tags(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for piece in raw.split(',') {
        let t = piece.trim().trim_matches(|c: char| c == '"' || c == '\'');
        if t.is_empty() {
            continue;
        }
        let lower = t.to_lowercase();
        if seen.insert(lower.clone()) {
            out.push(lower);
        }
    }
    out
}

/// Walk every node pair and emit an edge for any pair sharing
/// `>= MIN_EDGE_WEIGHT` tags. O(N²) over a few hundred nodes — fine at
/// startup and avoids the index bookkeeping of a tag → nodes inversion.
/// Output is sorted on `(source, target)` for stable JSON.
fn build_edges(nodes: &[GraphNode]) -> Vec<GraphEdge> {
    // Pre-build a tag set per node so the inner loop is a set intersection,
    // not an N-by-M list scan.
    let tag_sets: Vec<std::collections::HashSet<&str>> = nodes
        .iter()
        .map(|n| n.tags.iter().map(String::as_str).collect())
        .collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    for i in 0..nodes.len() {
        let a = &tag_sets[i];
        if a.is_empty() {
            continue;
        }
        for j in (i + 1)..nodes.len() {
            let b = &tag_sets[j];
            if b.is_empty() {
                continue;
            }
            // Intersect deterministically — iterate the smaller set and
            // collect by original order in `nodes[i].tags` so the output
            // is stable.
            let mut shared: Vec<String> = nodes[i]
                .tags
                .iter()
                .filter(|t| b.contains(t.as_str()))
                .cloned()
                .collect();
            shared.sort();
            shared.dedup();
            if (shared.len() as u32) < MIN_EDGE_WEIGHT {
                continue;
            }
            edges.push(GraphEdge {
                source: nodes[i].id.clone(),
                target: nodes[j].id.clone(),
                weight: shared.len() as u32,
                shared_tags: shared,
            });
        }
        // Tag-set lookups don't need `a` afterwards — drop the binding
        // explicitly so the silencer-pleasing _ is documented intent.
        let _ = a;
    }
    edges.sort_by(|x, y| {
        x.source
            .cmp(&y.source)
            .then_with(|| x.target.cmp(&y.target))
    });
    edges
}

/// Summary stats — used by the API endpoint envelope so a `curl` smoke
/// test reads naturally without piping through `jq`.
pub fn weight_histogram(edges: &[GraphEdge]) -> HashMap<u32, u32> {
    let mut hist = HashMap::new();
    for e in edges {
        *hist.entry(e.weight).or_insert(0) += 1;
    }
    hist
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, tier: &str, tags: &[&str]) -> DocumentInner {
        let mut fm = HashMap::new();
        fm.insert("tier".to_string(), tier.to_string());
        fm.insert("type".to_string(), "concept".to_string());
        fm.insert("confidence".to_string(), "high".to_string());
        fm.insert(
            "tags".to_string(),
            tags.join(", "),
        );
        DocumentInner {
            id: id.to_string(),
            title: id.rsplit('/').next().unwrap_or(id).to_string(),
            frontmatter: fm,
            body: String::new(),
        }
    }

    fn raw_doc(id: &str, frontmatter: Vec<(&str, &str)>) -> DocumentInner {
        let mut fm = HashMap::new();
        for (k, v) in frontmatter {
            fm.insert(k.to_string(), v.to_string());
        }
        DocumentInner {
            id: id.to_string(),
            title: id.to_string(),
            frontmatter: fm,
            body: String::new(),
        }
    }

    #[test]
    fn build_excludes_non_wiki_docs() {
        let docs = vec![
            doc("wikis/principles/concepts/a.md", "principles", &["x", "y"]),
            doc("sources/principles/foo.md", "principles", &["x", "y"]),
            doc("_meta/taxonomy.md", "", &[]),
        ];
        let g = CorpusGraph::build(&docs);
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].id, "wikis/principles/concepts/a.md");
    }

    #[test]
    fn edges_only_for_shared_tags_at_or_above_threshold() {
        // a, b share 3 tags (x, y, z) → edge weight 3
        // a, c share 1 tag (x) → no edge (below threshold)
        // b, c share 2 tags (x, q) → edge weight 2
        let docs = vec![
            doc(
                "wikis/principles/concepts/a.md",
                "principles",
                &["x", "y", "z", "p"],
            ),
            doc(
                "wikis/principles/concepts/b.md",
                "principles",
                &["x", "y", "z", "q"],
            ),
            doc("wikis/knowledge/concepts/c.md", "knowledge", &["x", "q", "r"]),
        ];
        let g = CorpusGraph::build(&docs);
        assert_eq!(g.nodes.len(), 3);
        let weights: HashMap<(&str, &str), u32> = g
            .edges
            .iter()
            .map(|e| ((e.source.as_str(), e.target.as_str()), e.weight))
            .collect();
        assert_eq!(
            weights
                .get(&(
                    "wikis/principles/concepts/a.md",
                    "wikis/principles/concepts/b.md"
                ))
                .copied(),
            Some(3)
        );
        assert_eq!(
            weights
                .get(&(
                    "wikis/knowledge/concepts/c.md",
                    "wikis/principles/concepts/b.md"
                ))
                .copied()
                .or_else(|| weights
                    .get(&(
                        "wikis/principles/concepts/b.md",
                        "wikis/knowledge/concepts/c.md"
                    ))
                    .copied()),
            Some(2)
        );
        // No edge between a and c (one shared tag only).
        assert!(!weights.contains_key(&(
            "wikis/knowledge/concepts/c.md",
            "wikis/principles/concepts/a.md"
        )));
        assert!(!weights.contains_key(&(
            "wikis/principles/concepts/a.md",
            "wikis/knowledge/concepts/c.md"
        )));
    }

    #[test]
    fn shared_tags_payload_lists_overlap() {
        let docs = vec![
            doc("wikis/principles/concepts/a.md", "principles", &["munger", "buffett"]),
            doc("wikis/principles/concepts/b.md", "principles", &["munger", "buffett", "duan"]),
        ];
        let g = CorpusGraph::build(&docs);
        assert_eq!(g.edges.len(), 1);
        let mut shared = g.edges[0].shared_tags.clone();
        shared.sort();
        assert_eq!(shared, vec!["buffett", "munger"]);
    }

    #[test]
    fn tier_falls_back_to_path_segment() {
        let docs = vec![raw_doc(
            "wikis/knowledge/topics/foo.md",
            vec![("tags", "x, y, z")],
        )];
        let g = CorpusGraph::build(&docs);
        assert_eq!(g.nodes[0].tier, "knowledge");
    }

    #[test]
    fn parse_tags_normalises_and_dedupes() {
        assert_eq!(parse_tags("a, b, A , c"), vec!["a", "b", "c"]);
        assert_eq!(parse_tags(" buffett ,  munger , buffett"), vec!["buffett", "munger"]);
        assert!(parse_tags("").is_empty());
        assert!(parse_tags("  ,  ,  ").is_empty());
    }

    #[test]
    fn nodes_carry_confidence_and_type() {
        let docs = vec![raw_doc(
            "wikis/principles/concepts/x.md",
            vec![
                ("type", "concept"),
                ("confidence", "high"),
                ("slug", "x"),
                ("tags", "a, b"),
            ],
        )];
        let g = CorpusGraph::build(&docs);
        let n = &g.nodes[0];
        assert_eq!(n.doc_type, "concept");
        assert_eq!(n.confidence, "high");
        assert_eq!(n.slug, "x");
        assert_eq!(n.directory_path, "wikis/principles/concepts");
    }

    #[test]
    fn empty_corpus_is_empty_graph() {
        let g = CorpusGraph::build(&[]);
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn nodes_with_no_tags_have_no_edges() {
        let docs = vec![
            doc("wikis/principles/concepts/a.md", "principles", &[]),
            doc("wikis/principles/concepts/b.md", "principles", &["x", "y"]),
            doc("wikis/principles/concepts/c.md", "principles", &["x", "y"]),
        ];
        let g = CorpusGraph::build(&docs);
        // Only the b ↔ c edge survives — a has no tags so no overlap.
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].weight, 2);
        assert!(!g
            .edges
            .iter()
            .any(|e| e.source.contains("/a.md") || e.target.contains("/a.md")));
    }

    #[test]
    fn weight_histogram_counts_by_overlap() {
        let docs = vec![
            doc("wikis/principles/concepts/a.md", "principles", &["x", "y"]),
            doc("wikis/principles/concepts/b.md", "principles", &["x", "y"]),
            doc("wikis/principles/concepts/c.md", "principles", &["x", "y", "z"]),
            doc("wikis/principles/concepts/d.md", "principles", &["x", "y", "z"]),
        ];
        let g = CorpusGraph::build(&docs);
        let hist = weight_histogram(&g.edges);
        // Pairs sharing exactly 2: (a,b), (a,c), (a,d), (b,c), (b,d) = 5
        // Pairs sharing exactly 3: (c,d) = 1
        assert_eq!(hist.get(&2).copied().unwrap_or(0), 5);
        assert_eq!(hist.get(&3).copied().unwrap_or(0), 1);
    }
}
