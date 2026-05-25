use std::fmt;
use std::time::Duration;

use reqwest::Client;
use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub const TUSHARE_ENDPOINT: &str = "https://api.tushare.pro";
pub const TUSHARE_TOKEN_ENV: &str = "TUSHARE_TOKEN";

const REQUEST_TIMEOUT_SECS: u64 = 20;

#[derive(Clone, Serialize)]
pub struct TushareRequest {
    pub api_name: String,
    token: String,
    pub params: Value,
    pub fields: String,
}

impl TushareRequest {
    pub fn new(
        api_name: impl Into<String>,
        token: impl Into<String>,
        params: Value,
        fields: impl Into<String>,
    ) -> Result<Self, TushareError> {
        let api_name = api_name.into().trim().to_string();
        let token = token.into().trim().to_string();
        if api_name.is_empty() {
            return Err(TushareError::Validation {
                code: None,
                message: "api_name is required".to_string(),
            });
        }
        if token.is_empty() {
            return Err(TushareError::MissingToken);
        }
        if !params.is_object() {
            return Err(TushareError::Validation {
                code: None,
                message: "params must be a JSON object".to_string(),
            });
        }

        Ok(Self {
            api_name,
            token,
            params,
            fields: fields.into().trim().to_string(),
        })
    }

    pub fn empty_params(
        api_name: impl Into<String>,
        token: impl Into<String>,
        fields: impl Into<String>,
    ) -> Result<Self, TushareError> {
        Self::new(api_name, token, Value::Object(Map::new()), fields)
    }
}

impl fmt::Debug for TushareRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TushareRequest")
            .field("api_name", &self.api_name)
            .field("token", &"<redacted>")
            .field("params", &self.params)
            .field("fields", &self.fields)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TushareResponse {
    pub code: i64,
    pub msg: String,
    pub fields: Vec<String>,
    pub items: Vec<Vec<Value>>,
}

impl TushareResponse {
    pub fn from_body(body: &str) -> Result<Self, TushareError> {
        let value: Value = serde_json::from_str(body).map_err(|e| TushareError::Provider {
            code: None,
            message: format!("invalid JSON response: {e}"),
        })?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, TushareError> {
        let code =
            value
                .get("code")
                .and_then(value_as_i64)
                .ok_or_else(|| TushareError::Provider {
                    code: None,
                    message: "response missing numeric code".to_string(),
                })?;
        let msg = value.get("msg").map(value_as_text).unwrap_or_default();

        if code != 0 {
            return Err(classify_error(code, msg));
        }

        let data = value.get("data").ok_or_else(|| TushareError::Provider {
            code: Some(code),
            message: "response missing data".to_string(),
        })?;
        let fields = parse_fields(data.get("fields"), code)?;
        let items = parse_items(data.get("items"), code)?;

        Ok(Self {
            code,
            msg,
            fields,
            items,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|field| field == name)
    }

    pub fn field<'row>(&self, row: &'row [Value], name: &str) -> Option<&'row Value> {
        self.field_index(name).and_then(|idx| row.get(idx))
    }

    pub fn field_text(&self, row: &[Value], name: &str) -> Option<String> {
        self.field(row, name).map(value_as_text)
    }

    pub fn rows_as_objects(&self) -> Vec<Map<String, Value>> {
        self.items
            .iter()
            .map(|row| {
                self.fields
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, name)| {
                        row.get(idx).cloned().map(|value| (name.clone(), value))
                    })
                    .collect()
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct TushareClient {
    endpoint: String,
    token: String,
    http: Client,
}

impl TushareClient {
    pub fn from_env() -> Result<Self, TushareError> {
        let token = std::env::var(TUSHARE_TOKEN_ENV).map_err(|_| TushareError::MissingToken)?;
        Self::new(token)
    }

    pub fn new(token: impl Into<String>) -> Result<Self, TushareError> {
        Self::with_endpoint(TUSHARE_ENDPOINT, token)
    }

    pub fn with_endpoint(
        endpoint: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, TushareError> {
        Self::with_endpoint_and_client(endpoint, token, default_http_client()?)
    }

    pub fn with_client(token: impl Into<String>, http: Client) -> Result<Self, TushareError> {
        Self::with_endpoint_and_client(TUSHARE_ENDPOINT, token, http)
    }

    pub fn with_endpoint_and_client(
        endpoint: impl Into<String>,
        token: impl Into<String>,
        http: Client,
    ) -> Result<Self, TushareError> {
        let endpoint = endpoint.into().trim().to_string();
        let token = token.into().trim().to_string();
        if endpoint.is_empty() {
            return Err(TushareError::Validation {
                code: None,
                message: "endpoint is required".to_string(),
            });
        }
        if token.is_empty() {
            return Err(TushareError::MissingToken);
        }

        Ok(Self {
            endpoint,
            token,
            http,
        })
    }

    pub fn request(
        &self,
        api_name: impl Into<String>,
        params: Value,
        fields: impl Into<String>,
    ) -> Result<TushareRequest, TushareError> {
        TushareRequest::new(api_name, self.token.clone(), params, fields)
    }

    pub async fn send(&self, request: &TushareRequest) -> Result<TushareResponse, TushareError> {
        self.send_inner(request, None).await
    }

    pub async fn send_cancelled(
        &self,
        request: &TushareRequest,
        cancel: &CancellationToken,
    ) -> Result<TushareResponse, TushareError> {
        self.send_inner(request, Some(cancel)).await
    }

    async fn send_inner(
        &self,
        request: &TushareRequest,
        cancel: Option<&CancellationToken>,
    ) -> Result<TushareResponse, TushareError> {
        let send = self.http.post(&self.endpoint).json(request).send();
        let response = match cancel {
            Some(cancel) => {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(cancelled_error("before Tushare request")),
                    response = send => response
                }
            }
            None => send.await,
        }
        .map_err(|e| TushareError::Provider {
            code: None,
            message: format!("Tushare request failed: {e}"),
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(TushareError::Provider {
                code: None,
                message: format!("Tushare returned HTTP {}", status.as_u16()),
            });
        }
        let parse = response.json::<Value>();
        let value = match cancel {
            Some(cancel) => {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(cancelled_error("during Tushare response parse")),
                    value = parse => value
                }
            }
            None => parse.await,
        }
        .map_err(|e| TushareError::Provider {
            code: None,
            message: format!("invalid Tushare JSON response: {e}"),
        })?;
        TushareResponse::from_value(value)
    }

    pub async fn query(
        &self,
        api_name: impl Into<String>,
        params: Value,
        fields: impl Into<String>,
    ) -> Result<TushareResponse, TushareError> {
        let request = self.request(api_name, params, fields)?;
        self.send(&request).await
    }

    pub async fn query_cancelled(
        &self,
        api_name: impl Into<String>,
        params: Value,
        fields: impl Into<String>,
        cancel: &CancellationToken,
    ) -> Result<TushareResponse, TushareError> {
        let request = self.request(api_name, params, fields)?;
        self.send_cancelled(&request, cancel).await
    }
}

impl fmt::Debug for TushareClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TushareClient")
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum TushareError {
    #[error("TUSHARE_TOKEN is not configured")]
    MissingToken,
    #[error("Tushare permission error (code={code:?}): {message}")]
    Permission { code: Option<i64>, message: String },
    #[error("Tushare rate limit error (code={code:?}): {message}")]
    RateLimit { code: Option<i64>, message: String },
    #[error("Tushare validation error (code={code:?}): {message}")]
    Validation { code: Option<i64>, message: String },
    #[error("Tushare provider error (code={code:?}): {message}")]
    Provider { code: Option<i64>, message: String },
}

fn parse_fields(value: Option<&Value>, code: i64) -> Result<Vec<String>, TushareError> {
    let fields = value
        .and_then(Value::as_array)
        .ok_or_else(|| TushareError::Provider {
            code: Some(code),
            message: "response data.fields must be an array".to_string(),
        })?;

    fields
        .iter()
        .map(|field| {
            field
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| TushareError::Provider {
                    code: Some(code),
                    message: "response data.fields must contain strings".to_string(),
                })
        })
        .collect()
}

fn parse_items(value: Option<&Value>, code: i64) -> Result<Vec<Vec<Value>>, TushareError> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| TushareError::Provider {
            code: Some(code),
            message: "response data.items must be an array".to_string(),
        })?;

    items
        .iter()
        .map(|row| {
            row.as_array()
                .cloned()
                .ok_or_else(|| TushareError::Provider {
                    code: Some(code),
                    message: "response data.items rows must be arrays".to_string(),
                })
        })
        .collect()
}

fn classify_error(code: i64, message: String) -> TushareError {
    let normalized = message.to_lowercase();
    if contains_any(
        &normalized,
        &["token is empty", "missing token", "token为空", "token 为空"],
    ) {
        return TushareError::MissingToken;
    }
    if contains_any(
        &normalized,
        &[
            "rate limit",
            "too many",
            "频率",
            "频次",
            "每分钟",
            "每小时",
            "最多访问",
        ],
    ) {
        return TushareError::RateLimit {
            code: Some(code),
            message,
        };
    }
    if contains_any(
        &normalized,
        &[
            "permission",
            "unauthorized",
            "forbidden",
            "无权限",
            "没有权限",
            "权限",
            "积分",
            "token无效",
            "token invalid",
        ],
    ) {
        return TushareError::Permission {
            code: Some(code),
            message,
        };
    }
    if contains_any(
        &normalized,
        &[
            "invalid",
            "validation",
            "parameter",
            "param",
            "参数",
            "字段",
            "格式",
            "不存在",
            "不能为空",
        ],
    ) {
        return TushareError::Validation {
            code: Some(code),
            message,
        };
    }

    TushareError::Provider {
        code: Some(code),
        message,
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn default_http_client() -> Result<Client, TushareError> {
    Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| TushareError::Provider {
            code: None,
            message: format!("building Tushare HTTP client failed: {e}"),
        })
}

fn cancelled_error(phase: &str) -> TushareError {
    TushareError::Provider {
        code: None,
        message: format!("aborted {phase}"),
    }
}

fn value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn value_as_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_body_and_extracts_fields() {
        let response = TushareResponse::from_body(
            r#"{
                "code": 0,
                "msg": "",
                "data": {
                    "fields": ["ts_code", "trade_date", "close", "vol"],
                    "items": [
                        ["000001.SZ", "20260520", 12.34, 12345.6],
                        ["000001.SZ", "20260519", null, "987.0"]
                    ]
                }
            }"#,
        )
        .unwrap();

        assert_eq!(response.code, 0);
        assert_eq!(
            response.fields,
            vec!["ts_code", "trade_date", "close", "vol"]
        );
        assert_eq!(response.items.len(), 2);
        assert_eq!(
            response
                .field_text(&response.items[0], "trade_date")
                .as_deref(),
            Some("20260520")
        );
        assert_eq!(
            response.field_text(&response.items[0], "close").as_deref(),
            Some("12.34")
        );
        assert_eq!(
            response.field_text(&response.items[1], "close").as_deref(),
            Some("")
        );
        assert!(response.field(&response.items[0], "missing").is_none());
    }

    #[test]
    fn empty_items_are_successful_response() {
        let response = TushareResponse::from_body(
            r#"{
                "code": 0,
                "msg": "",
                "data": {
                    "fields": ["ts_code"],
                    "items": []
                }
            }"#,
        )
        .unwrap();

        assert!(response.is_empty());
    }

    #[test]
    fn rows_can_be_converted_to_objects() {
        let response = TushareResponse::from_body(
            r#"{
                "code": 0,
                "msg": "",
                "data": {
                    "fields": ["ts_code", "close"],
                    "items": [["000001.SZ", 12.34]]
                }
            }"#,
        )
        .unwrap();

        let rows = response.rows_as_objects();
        assert_eq!(
            rows[0].get("ts_code").and_then(Value::as_str),
            Some("000001.SZ")
        );
        assert_eq!(rows[0].get("close").and_then(Value::as_f64), Some(12.34));
    }

    #[test]
    fn classifies_permission_error() {
        let error =
            TushareResponse::from_body(r#"{"code": -2001, "msg": "抱歉，您没有权限访问该接口"}"#)
                .unwrap_err();

        assert_eq!(
            error,
            TushareError::Permission {
                code: Some(-2001),
                message: "抱歉，您没有权限访问该接口".to_string()
            }
        );
    }

    #[test]
    fn classifies_rate_limit_error() {
        let error = TushareResponse::from_body(
            r#"{"code": -2002, "msg": "抱歉，您每分钟最多访问该接口200次"}"#,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            TushareError::RateLimit {
                code: Some(-2002),
                ..
            }
        ));
    }

    #[test]
    fn classifies_validation_error() {
        let error =
            TushareResponse::from_body(r#"{"code": -2003, "msg": "参数 ts_code 格式不正确"}"#)
                .unwrap_err();

        assert!(matches!(
            error,
            TushareError::Validation {
                code: Some(-2003),
                ..
            }
        ));
    }

    #[test]
    fn classifies_provider_error() {
        let error =
            TushareResponse::from_body(r#"{"code": -9999, "msg": "upstream failed"}"#).unwrap_err();

        assert!(matches!(
            error,
            TushareError::Provider {
                code: Some(-9999),
                ..
            }
        ));
    }

    #[test]
    fn rejects_malformed_success_shape() {
        let error = TushareResponse::from_body(
            r#"{
                "code": 0,
                "msg": "",
                "data": {"fields": ["ts_code"], "items": [1]}
            }"#,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            TushareError::Provider { code: Some(0), .. }
        ));
    }

    #[test]
    fn rejects_missing_token_without_leaking_token() {
        assert_eq!(
            TushareRequest::empty_params("daily", "", "ts_code").unwrap_err(),
            TushareError::MissingToken
        );
    }

    #[test]
    fn request_and_client_debug_redact_token() {
        let request =
            TushareRequest::empty_params("daily", "secret-token", "ts_code,close").unwrap();
        let client = TushareClient::new("secret-token").unwrap();

        assert!(!format!("{request:?}").contains("secret-token"));
        assert!(!format!("{client:?}").contains("secret-token"));
    }

    #[test]
    fn request_serializes_tushare_body() {
        let request = TushareRequest::new(
            "daily",
            "secret-token",
            serde_json::json!({"ts_code": "000001.SZ"}),
            "ts_code,close",
        )
        .unwrap();
        let body = serde_json::to_value(request).unwrap();

        assert_eq!(body["api_name"], "daily");
        assert_eq!(body["token"], "secret-token");
        assert_eq!(body["params"]["ts_code"], "000001.SZ");
        assert_eq!(body["fields"], "ts_code,close");
    }
}
