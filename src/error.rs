use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("rate limited")]
    RateLimited,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("internal error: {0}")]
    Internal(String),
    /// 同一張影像（相同 frame id）已在處理中。
    #[error("conflict: {0}")]
    Conflict(String),
    /// 處理佇列已滿：回 503＋`Retry-After`，讓客戶端退避。
    #[error("busy: {0}")]
    Busy(String),
}

/// OGC WMTS/OWS 規定的錯誤須以 ows:ExceptionReport XML 回應（用於 WMTS 端點）。
pub fn ows_exception(status: StatusCode, code: &str, text: &str) -> Response {
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ExceptionReport xmlns="http://www.opengis.net/ows/1.1" version="1.0.0">
  <Exception exceptionCode="{code}">
    <ExceptionText>{text}</ExceptionText>
  </Exception>
</ExceptionReport>"#,
        code = crate::xmlutil::escape(code),
        text = crate::xmlutil::escape(text)
    );
    (status, [("Content-Type", "text/xml")], body).into_response()
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // 一般 REST API（軌跡上傳等）回傳 JSON；WMTS 端點在各自 handler 內改用
        // ows_exception 以符合 OGC 規範，此處提供 API 共用的預設行為。
        let (status, code, msg) = match &self {
            AppError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "Unauthorized", m.clone()),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, "Forbidden", m.clone()),
            AppError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "RateLimited",
                "request rate limit exceeded".to_string(),
            ),
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, "NotFound", m.clone()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, "BadRequest", m.clone()),
            AppError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, "InternalError", m.clone()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "Conflict", m.clone()),
            AppError::Busy(m) => (StatusCode::SERVICE_UNAVAILABLE, "Busy", m.clone()),
        };
        let body = serde_json::json!({ "error": code, "message": msg });
        match self {
            AppError::Busy(_) | AppError::Conflict(_) => (status, [("Retry-After", "2")], axum::Json(body)).into_response(),
            _ => (status, axum::Json(body)).into_response(),
        }
    }
}
