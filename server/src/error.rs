//! API errors: every failure is `{"detail": "..."}` with an HTTP status, like the web app expects.
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub detail: String,
    pub extra: Option<Value>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        ApiError { status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), detail: detail.into(), extra: None }
    }
    pub fn with(mut self, extra: Value) -> Self {
        self.extra = Some(extra);
        self
    }
}

pub fn err(status: u16, detail: impl Into<String>) -> ApiError {
    ApiError::new(status, detail)
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "detail": self.detail });
        if let (Some(Value::Object(extra)), Value::Object(b)) = (self.extra, &mut body) {
            b.extend(extra);
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        tracing::error!("database: {e}");
        ApiError::new(500, "Database error")
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::new(500, e.to_string())
    }
}
