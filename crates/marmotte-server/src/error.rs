//! HTTP error type. Wraps [`marmotte_core::CoreError`] and renders RFC 7807
//! `application/problem+json` responses.
//
// Rust guideline compliant 2026-05-06

use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use marmotte_core::error::CoreError;
use serde::Serialize;

/// Server error wrapper that maps [`CoreError`] variants to HTTP status codes
/// and serializes them as RFC 7807 `application/problem+json` bodies.
#[derive(Debug)]
pub struct ApiError(pub CoreError);

impl<E> From<E> for ApiError
where
    CoreError: From<E>,
{
    fn from(e: E) -> Self {
        Self(CoreError::from(e))
    }
}

/// RFC 7807 problem detail body.
#[derive(Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    typ: &'a str,
    title: &'a str,
    status: u16,
    detail: String,
}

impl ApiError {
    /// Maps the inner [`CoreError`] variant to an HTTP status code and a
    /// short machine-readable title string.
    fn status_and_title(&self) -> (StatusCode, &'static str) {
        match self.0 {
            CoreError::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
            CoreError::Forbidden { .. } => (StatusCode::FORBIDDEN, "forbidden"),
            CoreError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            CoreError::Conflict { .. } => (StatusCode::CONFLICT, "conflict"),
            CoreError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid_input"),
            CoreError::Quota(_) => (StatusCode::INSUFFICIENT_STORAGE, "quota_exceeded"),
            CoreError::Migration(_)
            | CoreError::Database(_)
            | CoreError::Io(_)
            | CoreError::Crypto(_)
            | CoreError::Config(_)
            | CoreError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, title) = self.status_and_title();
        let body = Problem {
            typ: "about:blank",
            title,
            status: status.as_u16(),
            detail: self.0.to_string(),
        };
        let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
        (
            status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            bytes,
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn not_found_renders_problem_json() {
        let e = ApiError(CoreError::NotFound { what: "x".into() });
        let resp = e.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = to_bytes(resp.into_body(), 8192).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["title"], "not_found");
        assert_eq!(v["status"], 404);
    }

    #[test]
    fn unauthorized_maps_to_401() {
        let e = ApiError(CoreError::Unauthorized);
        assert_eq!(e.status_and_title().0, StatusCode::UNAUTHORIZED);
    }
}
