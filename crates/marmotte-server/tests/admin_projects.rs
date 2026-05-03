//! Tests for /api/v1/admin/projects.
//

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use marmotte_server::build_app;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;
use common::fixture;

fn admin(token: &str) -> String {
    format!("Bearer {token}")
}

async fn json_call(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, admin(token));
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app.oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

#[tokio::test]
async fn create_get_update_delete_project() {
    let f = fixture().await;
    let app = build_app(f.state);

    let (s, v) = json_call(
        app.clone(),
        "POST",
        "/api/v1/admin/projects",
        &f.admin_token,
        Some(json!({"name":"new-proj"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let id = v["id"].as_i64().unwrap();

    let (s, v) = json_call(
        app.clone(),
        "GET",
        &format!("/api/v1/admin/projects/{id}"),
        &f.admin_token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["name"], "new-proj");

    let (s, v) = json_call(
        app.clone(),
        "PATCH",
        &format!("/api/v1/admin/projects/{id}"),
        &f.admin_token,
        Some(json!({"quota_bytes": 9999})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["quota_bytes"], 9999);

    let (s, _) = json_call(
        app,
        "DELETE",
        &format!("/api/v1/admin/projects/{id}"),
        &f.admin_token,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn duplicate_name_returns_409() {
    let f = fixture().await;
    let app = build_app(f.state);
    let (s, _) = json_call(
        app,
        "POST",
        "/api/v1/admin/projects",
        &f.admin_token,
        Some(json!({"name":"acme"})),
    )
    .await;
    // Fixture already creates "acme"
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn unauthenticated_returns_401() {
    let f = fixture().await;
    let app = build_app(f.state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/admin/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
