//! Tests for the admin Bearer middleware.
//
// Rust guideline compliant 2026-05-06

use axum::{
    Extension, Router,
    extract::Request,
    http::{StatusCode, header},
    middleware::from_fn_with_state,
    response::IntoResponse,
    routing::get,
};
use marmotte_core::auth::AdminAuth;
use marmotte_server::{middleware::auth_admin, state::AppState};
use tower::ServiceExt;

mod common;
use common::fixture;

async fn ping(Extension(_a): Extension<AdminAuth>) -> impl IntoResponse {
    "pong"
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/ping", get(ping))
        .layer(from_fn_with_state(state.clone(), auth_admin::middleware))
        .with_state(state)
}

#[tokio::test]
async fn accepts_valid_bearer() {
    let f = fixture().await;
    let resp = app(f.state)
        .oneshot(
            Request::builder()
                .uri("/ping")
                .header(header::AUTHORIZATION, format!("Bearer {}", f.admin_token))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn rejects_missing_or_wrong() {
    let f = fixture().await;
    let r1 = app(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/ping")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::UNAUTHORIZED);

    let r2 = app(f.state)
        .oneshot(
            Request::builder()
                .uri("/ping")
                .header(header::AUTHORIZATION, "Bearer bogus")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::UNAUTHORIZED);
}
