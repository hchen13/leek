//! Serve the prebuilt corpus graph (nodes + edges) to the frontend.
//!
//! The graph is regenerated on demand via `leek corpus rebuild-graph` and
//! embedded into the binary by `build.rs` + `include_str!`. Live mutations
//! (corpus authoring) trigger a manual rebuild; we don't watch the corpus
//! directory at runtime.

use axum::Json;
use axum::extract::Query;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::agent::tools::corpus_search;

const GRAPH_JSON: &str = include_str!("../../assets/corpus.graph.json");

pub async fn graph_handler() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            // Build is content-addressed by commit; long cache + immutable
            // is overkill since clients hit this once per session, but a
            // short cache helps when the corpus-brain view re-mounts.
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        GRAPH_JSON,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct DocQuery {
    /// Wikilink-style id (e.g. `wikis/principles/concepts/margin-of-safety`).
    pub id: String,
}

/// Return a single corpus document's frontmatter + body. Used by the brain
/// widget's node-click preview and any other "show me this wiki" UI surface.
pub async fn doc_handler(Query(params): Query<DocQuery>) -> Response {
    match corpus_search::lookup_doc(&params.id) {
        Some(view) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "public, max-age=300")],
            Json(view),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "corpus doc not found",
                "id": params.id,
            })),
        )
            .into_response(),
    }
}
