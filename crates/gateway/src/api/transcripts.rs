//! F2 transcript endpoints — read access to the raw LLM-layer archive.
//!
//! Three endpoints, intentionally low-ceremony so PM debug can `curl`
//! them straight:
//!
//! - `GET /api/v1/sessions/{id}/transcripts` — list metadata
//!   (no bodies; `request_bytes` / `response_bytes` are sizes).
//! - `GET .../transcripts/{turn_id}/{iteration}/request`  — raw request body
//!   as `application/json`.
//! - `GET .../transcripts/{turn_id}/{iteration}/response` — raw SSE response
//!   bytes as `text/event-stream`.
//!
//! The raw endpoints deliberately serve bytes verbatim (no base64, no JSON
//! envelope) so `curl | jq` / `curl | grep` works without an unwrap layer.
//! No auth: this is the same trust boundary as the rest of the gateway API.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;

use super::{ApiError, ApiResult, AppState};
use crate::vault::llm_transcripts;

/// `GET /api/v1/sessions/{id}/transcripts`
///
/// Returns `{ items: [TranscriptMeta…] }` ordered by `(turn_id, iteration)`.
/// An unknown session id returns an empty list (no separate 404 — the
/// archive itself is the source of truth).
pub async fn list(
    State(st): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = llm_transcripts::list_by_session(&st.pool, &session_id).await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

/// `GET /api/v1/sessions/{id}/transcripts/{turn_id}/{iteration}/request`
///
/// Streams the request body bytes verbatim under `application/json`.
pub async fn request_body(
    State(st): State<AppState>,
    Path((session_id, turn_id, iteration)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let Some(bytes) =
        llm_transcripts::get_request_body(&st.pool, &session_id, &turn_id, iteration).await?
    else {
        return Err(ApiError::not_found(format!(
            "no transcript for turn '{turn_id}' iteration {iteration} in session '{session_id}'"
        )));
    };
    raw_response("application/json", bytes)
}

/// `GET /api/v1/sessions/{id}/transcripts/{turn_id}/{iteration}/response`
///
/// Streams the response bytes verbatim under `text/event-stream`. An
/// unfinalized row returns an empty body (and status 200) — the row
/// exists, the stream simply never finished.
pub async fn response_stream(
    State(st): State<AppState>,
    Path((session_id, turn_id, iteration)): Path<(String, String, i64)>,
) -> ApiResult<Response> {
    let Some(bytes) =
        llm_transcripts::get_response_stream(&st.pool, &session_id, &turn_id, iteration).await?
    else {
        return Err(ApiError::not_found(format!(
            "no transcript for turn '{turn_id}' iteration {iteration} in session '{session_id}'"
        )));
    };
    raw_response("text/event-stream", bytes)
}

fn raw_response(content_type: &'static str, bytes: Vec<u8>) -> ApiResult<Response> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .map_err(|e| anyhow::anyhow!("building raw transcript response: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::config::Config;
    use crate::llm::codex::CodexClient;
    use crate::vault::{llm_transcripts, Vault};
    use axum::body::to_bytes;
    // `IntoResponse` is only used by the 404 case below — kept local so the
    // non-test build doesn't see an unused-import warning.
    use axum::response::IntoResponse;
    use std::sync::RwLock;

    /// Spin up an isolated vault on a uuid-named temp file and bolt an
    /// `AppState` around it. The state's codex client never makes a network
    /// call here — we only exercise the transcript handlers — so the lack
    /// of stored credentials is fine.
    async fn fixture_state() -> AppState {
        let path = std::env::temp_dir().join(format!(
            "leek-api-transcripts-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let vault = Vault::open(&path).await.unwrap();
        // Add the session that the handlers will reference.
        sqlx::query(
            "INSERT INTO sessions (id, user_id, title, created_at, last_active_at) \
             VALUES (?, ?, NULL, ?, ?)",
        )
        .bind("s-api")
        .bind(crate::vault::LOCAL_USER)
        .bind("2026-05-20T00:00:00Z")
        .bind("2026-05-20T00:00:00Z")
        .execute(&vault.pool)
        .await
        .unwrap();
        let codex = CodexClient::new(vault.pool.clone(), crate::vault::LOCAL_USER).unwrap();
        let corpus = std::sync::Arc::new(crate::corpus::Corpus::empty());
        let corpus_graph = std::sync::Arc::new(corpus.build_graph());
        AppState {
            pool: vault.pool,
            bus: EventBus::new(),
            codex,
            http: reqwest::Client::new(),
            config: std::sync::Arc::new(RwLock::new(Config::default())),
            web_search: false,
            corpus,
            corpus_graph,
        }
    }

    #[tokio::test]
    async fn list_returns_empty_for_unknown_session() {
        let state = fixture_state().await;
        let resp = list(State(state), Path("does-not-exist".to_string()))
            .await
            .unwrap()
            .0;
        assert_eq!(resp["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_returns_metadata_after_insert() {
        let state = fixture_state().await;
        let row_id = llm_transcripts::insert_request(
            &state.pool,
            "s-api",
            "t-1",
            1,
            "gpt-5.5",
            b"REQBODY",
            "2026-05-20T00:00:00Z",
        )
        .await
        .unwrap();
        llm_transcripts::finalize(
            &state.pool,
            row_id,
            b"data: hi\n\n",
            200,
            "2026-05-20T00:00:01Z",
        )
        .await
        .unwrap();

        let resp = list(State(state), Path("s-api".to_string()))
            .await
            .unwrap()
            .0;
        let items = resp["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        let m = &items[0];
        assert_eq!(m["turn_id"], "t-1");
        assert_eq!(m["iteration"], 1);
        assert_eq!(m["model"], "gpt-5.5");
        assert_eq!(m["http_status"], 200);
        assert_eq!(m["request_bytes"], "REQBODY".len() as i64);
        assert_eq!(m["response_bytes"], "data: hi\n\n".len() as i64);
        // List entries carry sizes, not the raw bodies themselves.
        assert!(m.get("request_body").is_none());
        assert!(m.get("response_stream").is_none());
    }

    #[tokio::test]
    async fn request_body_serves_raw_bytes_as_json() {
        let state = fixture_state().await;
        llm_transcripts::insert_request(
            &state.pool,
            "s-api",
            "t-1",
            1,
            "m",
            b"{\"hello\":\"world\"}",
            "ts",
        )
        .await
        .unwrap();
        let resp = request_body(
            State(state),
            Path(("s-api".to_string(), "t-1".to_string(), 1)),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/json"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"hello":"world"}"#);
    }

    #[tokio::test]
    async fn response_stream_serves_raw_bytes_as_event_stream() {
        let state = fixture_state().await;
        let row_id = llm_transcripts::insert_request(
            &state.pool,
            "s-api",
            "t-1",
            1,
            "m",
            b"req",
            "ts",
        )
        .await
        .unwrap();
        llm_transcripts::finalize(
            &state.pool,
            row_id,
            b"event: x\ndata: {\"a\":1}\n\n",
            200,
            "ts",
        )
        .await
        .unwrap();
        let resp = response_stream(
            State(state),
            Path(("s-api".to_string(), "t-1".to_string(), 1)),
        )
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/event-stream"
        );
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"event: x\ndata: {\"a\":1}\n\n");
    }

    #[tokio::test]
    async fn missing_iteration_returns_not_found() {
        let state = fixture_state().await;
        let resp = request_body(
            State(state),
            Path(("s-api".to_string(), "t-x".to_string(), 99)),
        )
        .await;
        // ApiError doesn't implement PartialEq — the error path is the
        // expected one and the IntoResponse conversion will give a 404.
        match resp {
            Err(e) => {
                let r = e.into_response();
                assert_eq!(r.status(), StatusCode::NOT_FOUND);
            }
            Ok(_) => panic!("expected 404, got 200"),
        }
    }
}
