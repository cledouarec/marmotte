//! Tests for /api/v1/admin/tokens.
//
// Rust guideline compliant 2026-05-06

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use marmotte_server::build_app;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;
use common::fixture;

#[tokio::test]
async fn create_token_returns_secret_once() {
    let f = fixture().await;
    let app = build_app(f.state);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/admin/tokens")
        .header(header::AUTHORIZATION, format!("Bearer {}", f.admin_token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({"label":"ci"})).unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert!(v["secret"].as_str().unwrap().len() >= 40);
}

#[tokio::test]
async fn revoked_token_cannot_authenticate() {
    let f = fixture().await;
    let app = build_app(f.state.clone());
    // Find the token id (fixture inserts label="root").
    let tokens = f.state.db.admin_tokens().list().await.unwrap();
    let id = tokens[0].id;

    // Use the still-active token to revoke itself.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/admin/tokens/{id}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Same token now blocked.
    let req = Request::builder()
        .uri("/api/v1/admin/projects")
        .header(header::AUTHORIZATION, format!("Bearer {}", f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
