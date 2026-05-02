//! Serve the prebuilt corpus graph (nodes + edges) to the frontend.
//!
//! The graph is regenerated on demand via `leek corpus rebuild-graph` and
//! embedded into the binary by `build.rs` + `include_str!`. Live mutations
//! (corpus authoring) trigger a manual rebuild; we don't watch the corpus
//! directory at runtime.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

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
