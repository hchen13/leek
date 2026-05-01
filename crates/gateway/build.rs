//! Build-time helper that ensures `frontend/web/dist/` exists with at least a
//! placeholder `index.html` so `rust_embed`'s macro doesn't fail during
//! `cargo build` when the frontend hasn't been built yet.
//!
//! Production: run `cd frontend/web && npm run build` before `cargo build --release`.
//! Dev: ignore this file — the gateway uses dev-mode embed which reads from disk.

use std::fs;
use std::path::Path;

const PLACEHOLDER: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>L.E.E.K — frontend not built</title>
<body style="font-family: system-ui; padding: 40px; max-width: 640px; margin: 0 auto;">
<h1>Frontend not built.</h1>
<p>Run <code>cd frontend/web &amp;&amp; npm run build</code> then rebuild the gateway.</p>
</body>"#;

fn main() {
    let dist = Path::new("../../frontend/web/dist");
    if !dist.join("index.html").exists() {
        if let Err(e) = fs::create_dir_all(dist) {
            eprintln!("warning: could not create dist placeholder: {e}");
        }
        if let Err(e) = fs::write(dist.join("index.html"), PLACEHOLDER) {
            eprintln!("warning: could not write dist placeholder: {e}");
        }
    }
    // Re-run build script when frontend changes
    println!("cargo:rerun-if-changed=../../frontend/web/dist/index.html");
}
