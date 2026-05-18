//! Health check.

use axum::Json;

pub async fn handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "service": "leek-gateway",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
