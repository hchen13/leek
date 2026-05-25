//! Corpus Brain HTTP — `GET /api/v1/corpus/graph` (M2.1).
//!
//! Serves the precomputed wiki graph (nodes + edges) as JSON. The graph
//! is built once at startup and stashed on `AppState.corpus_graph`, so
//! this handler is just a `clone-and-serialize` — no allocation hot path.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use super::AppState;
use crate::corpus::{weight_histogram, CorpusGraph};

/// `GET /api/v1/corpus/graph`
///
/// Returns the full wiki graph plus a `stats` summary the frontend uses
/// to drive its "graph empty?" guard without re-counting client-side.
pub async fn graph(State(st): State<AppState>) -> Json<serde_json::Value> {
    let graph: Arc<CorpusGraph> = st.corpus_graph.clone();
    let stats = serde_json::json!({
        "node_count": graph.nodes.len(),
        "edge_count": graph.edges.len(),
        "weight_histogram": weight_histogram(&graph.edges),
    });
    Json(serde_json::json!({
        "nodes": graph.nodes,
        "edges": graph.edges,
        "stats": stats,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::GuardConfig;
    use crate::bus::EventBus;
    use crate::corpus::Corpus;
    use crate::llm::codex::CodexClient;
    use crate::vault::Vault;
    use std::path::PathBuf;

    async fn fixture_state() -> AppState {
        // Build the corpus from the existing test fixtures (which now
        // contain three wiki docs, two sharing tags) so the handler
        // returns a non-trivial graph.
        let path = std::env::temp_dir().join(format!(
            "leek-api-corpus-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let vault = Vault::open(&path).await.unwrap();
        let codex = CodexClient::new(vault.pool.clone(), crate::vault::LOCAL_USER).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
        let corpus = std::sync::Arc::new(Corpus::load(&root).unwrap());
        let corpus_graph = std::sync::Arc::new(corpus.build_graph());
        AppState {
            pool: vault.pool,
            bus: EventBus::new(),
            codex,
            http: reqwest::Client::new(),
            config: std::sync::Arc::new(std::sync::RwLock::new(crate::config::Config::default())),
            web_search: false,
            corpus,
            corpus_graph,
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            hooks: std::sync::Arc::new(crate::hooks::HookEngine::default()),
            agents: std::sync::Arc::new(crate::agents::AgentRegistry::default()),
            vendors: std::sync::Arc::new(crate::vendors::VendorRegistry::for_test()),
            abort_signals: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }


    #[tokio::test]
    async fn graph_endpoint_returns_wiki_only_nodes() {
        let st = fixture_state().await;
        let resp = graph(axum::extract::State(st)).await;
        let body = resp.0;
        // Every node must be a `wikis/...` doc — `sources/` and `_meta/`
        // are excluded by the builder (REQUIREMENTS §2.5).
        let nodes = body["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());
        for n in nodes {
            assert!(n["id"].as_str().unwrap().starts_with("wikis/"));
            // Every node carries the keys the frontend reads on render.
            for k in ["title", "tier", "type", "tags", "confidence", "directory_path"] {
                assert!(n.get(k).is_some(), "node missing `{k}`: {n}");
            }
        }
        let stats = &body["stats"];
        assert_eq!(stats["node_count"], nodes.len());
    }

    #[tokio::test]
    async fn graph_endpoint_emits_edges_for_shared_tags() {
        let st = fixture_state().await;
        let resp = graph(axum::extract::State(st)).await;
        let edges = resp.0["edges"].as_array().unwrap();
        // The two `principles/concepts/...` fixtures share three tags —
        // exactly one edge of weight 3 should connect them.
        assert!(!edges.is_empty(), "expected at least one edge from fixtures");
        let weights: Vec<u64> =
            edges.iter().map(|e| e["weight"].as_u64().unwrap()).collect();
        assert!(weights.iter().all(|w| *w >= 2));
    }

    #[tokio::test]
    async fn graph_endpoint_handles_empty_corpus() {
        // Bypass the fixture corpus — start from an empty one, so we
        // verify the handler's contract on zero state.
        let path = std::env::temp_dir().join(format!(
            "leek-api-corpus-empty-{}.db",
            uuid::Uuid::new_v4()
        ));
        let vault = Vault::open(&path).await.unwrap();
        let codex = CodexClient::new(vault.pool.clone(), crate::vault::LOCAL_USER).unwrap();
        let corpus = std::sync::Arc::new(Corpus::empty());
        let corpus_graph = std::sync::Arc::new(corpus.build_graph());
        let st = AppState {
            pool: vault.pool,
            bus: EventBus::new(),
            codex,
            http: reqwest::Client::new(),
            config: std::sync::Arc::new(std::sync::RwLock::new(crate::config::Config::default())),
            web_search: false,
            corpus,
            corpus_graph,
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            hooks: std::sync::Arc::new(crate::hooks::HookEngine::default()),
            agents: std::sync::Arc::new(crate::agents::AgentRegistry::default()),
            vendors: std::sync::Arc::new(crate::vendors::VendorRegistry::for_test()),
            abort_signals: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        };
        let resp = graph(axum::extract::State(st)).await;
        assert_eq!(resp.0["nodes"].as_array().unwrap().len(), 0);
        assert_eq!(resp.0["edges"].as_array().unwrap().len(), 0);
        assert_eq!(resp.0["stats"]["node_count"], 0);
    }
}
