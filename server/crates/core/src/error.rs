//! API errors: every failure is `{"detail": "..."}` with an HTTP status, like the web app expects.
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

/// A failed request: the HTTP status and the `detail` the client shows.
#[derive(Debug)]
pub struct ApiError {
    /// HTTP status of the response
    pub status: StatusCode,
    /// the message, sent as `{"detail": …}`
    pub detail: String,
    /// more fields merged into the error body
    pub extra: Option<Value>,
}

/// What a handler returns.
pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    /// An error with HTTP `status` (500 when not a valid code) and `detail`.
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        ApiError { status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), detail: detail.into(), extra: None }
    }
    /// The same error with `extra` fields in its body.
    pub fn with(mut self, extra: Value) -> Self {
        self.extra = Some(extra);
        self
    }
}

/// Shorthand for [`ApiError::new`].
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
