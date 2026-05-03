//! Smoke tests for the public route group.
//!
//! Each test uses an in-process axum harness via `tower::ServiceExt::oneshot`
//! so no network socket is opened.
//
// Rust guideline compliant 2026-05-06

use axum::{body::to_bytes, http::Request};
use marmotte_server::build_app;
use tower::ServiceExt;

mod common;
use common::fixture;

#[tokio::test]
async fn healthz_returns_ok() {
    let app = build_app(fixture().await.state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"ok\n");
}

#[tokio::test]
async fn readyz_returns_ready() {
    let app = build_app(fixture().await.state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/readyz")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn metrics_endpoint_lists_marmotte_metrics() {
    let app = build_app(fixture().await.state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let s = std::str::from_utf8(&body).unwrap();
    assert!(s.contains("marmotte_http_requests_total"));
    assert!(s.contains("marmotte_storage_blobs"));
}
