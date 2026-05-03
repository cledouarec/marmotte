//! Tests for API-key endpoints.
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

fn admin(t: &str) -> String {
    format!("Bearer {t}")
}

#[tokio::test]
async fn create_then_list_then_revoke() {
    let f = fixture().await;
    let pid = f
        .state
        .db
        .projects()
        .get_by_name(&f.project_name)
        .await
        .unwrap()
        .id;
    let app = build_app(f.state);

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/projects/{pid}/keys"))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({"role":"read","label":"l"})).unwrap(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let kid = body["id"].as_i64().unwrap();
    assert!(body["secret"].as_str().unwrap().len() >= 40);

    let req = Request::builder()
        .uri(format!("/api/v1/admin/projects/{pid}/keys"))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let arr = v.as_array().unwrap();
    assert!(arr.len() >= 2); // fixture key + the one we just made
    // Listed keys must NOT carry secrets.
    assert!(arr.iter().all(|k| k.get("secret").is_none()));

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/admin/projects/{pid}/keys/{kid}"))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}
