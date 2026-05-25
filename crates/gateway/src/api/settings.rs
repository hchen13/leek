use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::vault::data_provider_configs;

use super::{AppError, AppState};

const TUSHARE_PROVIDER: &str = "tushare";

#[derive(Serialize)]
pub struct SettingsResponse {
    pub data_providers: Vec<data_provider_configs::DataProviderStatus>,
}

pub async fn get_handler(
    State(state): State<AppState>,
) -> Result<Json<SettingsResponse>, AppError> {
    let mut providers = Vec::new();
    if let Some(status) =
        data_provider_configs::get_status(&state.pool, &state.user_id, TUSHARE_PROVIDER).await?
    {
        providers.push(status);
    } else {
        providers.push(default_tushare_status());
    }
    Ok(Json(SettingsResponse {
        data_providers: providers,
    }))
}

#[derive(Deserialize)]
pub struct PutTushareBody {
    pub token: Option<String>,
    pub enabled: Option<bool>,
}

pub async fn put_tushare_handler(
    State(state): State<AppState>,
    Json(body): Json<PutTushareBody>,
) -> Result<axum::response::Response, AppError> {
    let enabled = body.enabled.unwrap_or(true);
    let status = match body.token {
        Some(token) => {
            let token = token.trim();
            if token.is_empty() {
                return Ok((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "code": "INVALID_TUSHARE_TOKEN",
                            "message": "Tushare token 不能为空"
                        }
                    })),
                )
                    .into_response());
            }
            data_provider_configs::upsert_api_key(
                &state.pool,
                &state.user_id,
                TUSHARE_PROVIDER,
                token,
                enabled,
            )
            .await?
        }
        None => {
            match data_provider_configs::set_enabled(
                &state.pool,
                &state.user_id,
                TUSHARE_PROVIDER,
                enabled,
            )
            .await?
            {
                Some(status) => status,
                None => {
                    let Some(env_token) = env_tushare_token() else {
                        return Ok((
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": {
                                    "code": "TUSHARE_TOKEN_NOT_CONFIGURED",
                                    "message": "请先填写 Tushare token"
                                }
                            })),
                        )
                            .into_response());
                    };
                    if enabled {
                        env_tushare_status(&env_token)
                    } else {
                        data_provider_configs::upsert_api_key(
                            &state.pool,
                            &state.user_id,
                            TUSHARE_PROVIDER,
                            "",
                            false,
                        )
                        .await?
                    }
                }
            }
        }
    };

    Ok(Json(serde_json::json!({ "provider": status })).into_response())
}

fn default_tushare_status() -> data_provider_configs::DataProviderStatus {
    match env_tushare_token() {
        Some(token) => env_tushare_status(&token),
        None => data_provider_configs::DataProviderStatus {
            provider_name: TUSHARE_PROVIDER.to_string(),
            source: "none".to_string(),
            configured: false,
            enabled: true,
            token_last4: None,
            updated_at: String::new(),
            last_error: None,
            last_error_at: None,
        },
    }
}

fn env_tushare_status(token: &str) -> data_provider_configs::DataProviderStatus {
    data_provider_configs::DataProviderStatus {
        provider_name: TUSHARE_PROVIDER.to_string(),
        source: "env".to_string(),
        configured: true,
        enabled: true,
        token_last4: Some(last4(token)),
        updated_at: String::new(),
        last_error: None,
        last_error_at: None,
    }
}

fn env_tushare_token() -> Option<String> {
    std::env::var("TUSHARE_TOKEN")
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn last4(token: &str) -> String {
    let mut chars: Vec<char> = token.chars().rev().take(4).collect();
    chars.reverse();
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use futures::stream::{self, BoxStream};
    use tower::ServiceExt;

    use crate::agent::tools::ToolRegistry;
    use crate::api::{AppState, router};
    use crate::events::EventBus;
    use crate::llm::{ChatRequest, LlmEvent, LlmProvider};

    struct NullProvider;

    #[async_trait]
    impl LlmProvider for NullProvider {
        fn name(&self) -> &str {
            "null"
        }

        async fn chat(
            &self,
            _req: ChatRequest,
        ) -> anyhow::Result<BoxStream<'static, anyhow::Result<LlmEvent>>> {
            Ok(Box::pin(stream::empty()))
        }
    }

    async fn test_state() -> AppState {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE data_provider_configs (
                user_id TEXT NOT NULL,
                provider_name TEXT NOT NULL,
                api_key TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_error TEXT,
                last_error_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, provider_name)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        AppState {
            pool,
            provider: Arc::new(NullProvider),
            event_bus: EventBus::new(),
            user_id: "u".to_string(),
            active_replies: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            tools: ToolRegistry::empty(),
        }
    }

    async fn request(method: Method, path: &str, body: &str) -> (axum::http::StatusCode, String) {
        let app = router(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn saving_token_returns_last4_without_full_token() {
        let (status, body) = request(
            Method::PUT,
            "/api/v1/settings/tushare",
            r#"{"token":"abc123456789","enabled":true}"#,
        )
        .await;

        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(body.contains(r#""token_last4":"6789""#));
        assert!(!body.contains("abc123456789"));
    }

    #[tokio::test]
    async fn empty_token_is_rejected() {
        let (status, body) = request(
            Method::PUT,
            "/api/v1/settings/tushare",
            r#"{"token":"   ","enabled":true}"#,
        )
        .await;

        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(body.contains("INVALID_TUSHARE_TOKEN"));
    }

    #[tokio::test]
    async fn toggling_without_saved_token_is_rejected() {
        let (status, body) = request(
            Method::PATCH,
            "/api/v1/settings/tushare",
            r#"{"enabled":false}"#,
        )
        .await;

        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(body.contains("TUSHARE_TOKEN_NOT_CONFIGURED"));
    }
}
