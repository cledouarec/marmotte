//! Tests for the project Basic-auth middleware.
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
use base64::{Engine, engine::general_purpose::STANDARD};
use marmotte_core::auth::ProjectAuth;
use marmotte_server::{middleware::auth_project, state::AppState};
use tower::ServiceExt;

mod common;
use common::fixture;

async fn echo_role(Extension(p): Extension<ProjectAuth>) -> impl IntoResponse {
    p.role.as_str().to_string()
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/echo", get(echo_role))
        .layer(from_fn_with_state(state.clone(), auth_project::middleware))
        .with_state(state)
}

fn basic(user: &str, pass: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
}

#[tokio::test]
async fn rejects_missing_credentials() {
    let f = fixture().await;
    let resp = app(f.state)
        .oneshot(
            Request::builder()
                .uri("/echo")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn accepts_valid_basic_credentials() {
    let f = fixture().await;
    let auth = basic(&f.project_name, &f.api_key);
    let resp = app(f.state)
        .oneshot(
            Request::builder()
                .uri("/echo")
                .header(header::AUTHORIZATION, auth)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn rejects_wrong_password() {
    let f = fixture().await;
    let auth = basic(&f.project_name, "wrong");
    let resp = app(f.state)
        .oneshot(
            Request::builder()
                .uri("/echo")
                .header(header::AUTHORIZATION, auth)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
