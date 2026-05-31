//! Build-time helper that ensures generated assets exist before the main
//! crate compiles, so `include_str!` / `RustEmbed` macros don't fail:
//!
//! 1. `frontend/web/dist/index.html` — placeholder when the SolidJS app
//!    hasn't been built (production: `cd frontend/web && npm run build`).
//! 2. `crates/gateway/assets/corpus.graph.json` — placeholder when the
//!    corpus hasn't been crawled yet (production: `leek corpus rebuild-graph`).
//!
//! We do NOT re-crawl the corpus from build.rs — that would require a
//! build-deps crate split. Users run `leek corpus rebuild-graph` whenever they
//! want a fresh graph; this script just guarantees the macro doesn't blow up on
//! a fresh checkout. The runtime kernel is no longer a build asset: the
//! system-prompt builder reads it live from the embedded corpus.

use std::fs;
use std::path::Path;

const FRONTEND_PLACEHOLDER: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>L.E.E.K — frontend not built</title>
<body style="font-family: system-ui; padding: 40px; max-width: 640px; margin: 0 auto;">
<h1>Frontend not built.</h1>
<p>Run <code>cd frontend/web &amp;&amp; npm run build</code> then rebuild the gateway.</p>
</body>"#;

const EMPTY_CORPUS_GRAPH: &str = r#"{
  "version": 1,
  "generated_at": "1970-01-01T00:00:00Z",
  "nodes": [],
  "edges": []
}
"#;

fn main() {
    let dist = Path::new("../../frontend/web/dist");
    if !dist.join("index.html").exists() {
        if let Err(e) = fs::create_dir_all(dist) {
            eprintln!("warning: could not create dist placeholder: {e}");
        }
        if let Err(e) = fs::write(dist.join("index.html"), FRONTEND_PLACEHOLDER) {
            eprintln!("warning: could not write dist placeholder: {e}");
        }
    }

    let corpus_graph = Path::new("assets/corpus.graph.json");
    if !corpus_graph.exists() {
        if let Some(parent) = corpus_graph.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("warning: could not create assets/: {e}");
            }
        }
        if let Err(e) = fs::write(corpus_graph, EMPTY_CORPUS_GRAPH) {
            eprintln!("warning: could not write corpus.graph.json placeholder: {e}");
        }
    }

    println!("cargo:rerun-if-changed=../../frontend/web/dist/index.html");
    println!("cargo:rerun-if-changed=assets/corpus.graph.json");
}
