//! Tests for /api/v1/admin/projects/{id}/entries.
//

use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use marmotte_server::build_app;
use serde_json::Value;
use tower::ServiceExt;

mod common;
use common::fixture;

fn admin(t: &str) -> String {
    format!("Bearer {t}")
}
fn basic(u: &str, p: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{u}:{p}")))
}

async fn put_entry(app: &axum::Router, project: &str, key: &str, path: &str, body: &'static [u8]) {
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/sstate/{project}/{path}"))
        .header(header::AUTHORIZATION, basic(project, key))
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert!(resp.status().is_success(), "PUT failed: {}", resp.status());
}

#[tokio::test]
async fn list_paginates_and_filters() {
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

    for body in [b"a" as &[u8], b"bb", b"ccc", b"dddd"] {
        put_entry(
            &app,
            &f.project_name,
            &f.api_key,
            &format!("p/{}", body.len()),
            body,
        )
        .await;
    }

    let req = Request::builder()
        .uri(format!(
            "/api/v1/admin/projects/{pid}/entries?limit=2&sort=path&order=asc"
        ))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    let cursor = v["next_cursor"].as_str().unwrap().to_string();

    let req = Request::builder()
        .uri(format!(
            "/api/v1/admin/projects/{pid}/entries?limit=10&sort=path&order=asc&cursor={cursor}"
        ))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let v: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    assert_eq!(v["has_more"], false);
}

#[tokio::test]
async fn pin_then_delete_blocked_by_gc_only() {
    let f = fixture().await;
    let pid = f
        .state
        .db
        .projects()
        .get_by_name(&f.project_name)
        .await
        .unwrap()
        .id;
    let app = build_app(f.state.clone());
    put_entry(&app, &f.project_name, &f.api_key, "x", b"x").await;
    let entries = f
        .state
        .db
        .entries()
        .list(pid, marmotte_core::db::entries::ListFilter::default())
        .await
        .unwrap();
    let eid = entries.entries[0].id;

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/projects/{pid}/entries/{eid}/pin"))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);

    // Manual DELETE still works (pin only protects from GC).
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/admin/projects/{pid}/entries/{eid}"))
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test]
async fn gc_run_returns_zero_on_empty() {
    let f = fixture().await;
    let app = build_app(f.state);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/admin/gc/run?dry_run=true")
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(v["dry_run"], true);
}

#[tokio::test]
async fn stats_endpoint_returns_global_summary() {
    let f = fixture().await;
    let app = build_app(f.state);
    let req = Request::builder()
        .uri("/api/v1/admin/stats")
        .header(header::AUTHORIZATION, admin(&f.admin_token))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert!(v["projects"].as_i64().unwrap() >= 1);
}
