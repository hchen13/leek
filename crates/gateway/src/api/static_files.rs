//! Static asset handler — serves `frontend/web/dist/` embedded into the binary.
//!
//! The fallback rule is SPA-friendly: any path that doesn't match an embedded
//! asset returns `index.html` so client-side routing (URL hashes / paths) works
//! even on cold reload.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../frontend/web/dist"]
struct Asset;

pub async fn handler(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let path = if raw.is_empty() { "index.html" } else { raw };

    // 1. Try exact path
    if let Some(file) = Asset::get(path) {
        return serve(path, file.data.into_owned());
    }

    // 2. SPA fallback — serve index.html for unknown paths
    if let Some(file) = Asset::get("index.html") {
        return serve("index.html", file.data.into_owned());
    }

    (StatusCode::NOT_FOUND, "frontend not embedded").into_response()
}

fn serve(path: &str, body: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(body))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "building static response failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "static serve failed").into_response()
        })
}
