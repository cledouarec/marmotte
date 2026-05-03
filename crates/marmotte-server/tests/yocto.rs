//! End-to-end tests for the Yocto routes.
//
// Rust guideline compliant 2026-05-06

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use marmotte_server::build_app;
use tower::ServiceExt;

mod common;
use common::fixture;

fn basic(u: &str, p: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{u}:{p}")))
}

#[tokio::test]
async fn put_then_get_round_trip() {
    let f = fixture().await;
    let app = build_app(f.state.clone());

    // PUT
    let auth = basic(&f.project_name, &f.api_key);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/sstate/acme/foo/bar.tar.zst")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::from("hello world"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // GET
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/sstate/acme/foo/bar.tar.zst")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let etag = resp.headers().get(header::ETAG).cloned();
    let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    assert_eq!(&body[..], b"hello world");
    assert!(etag.is_some());
}

#[tokio::test]
async fn cross_project_url_returns_forbidden() {
    let f = fixture().await;
    let app = build_app(f.state);
    let auth = basic(&f.project_name, &f.api_key);
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/sstate/other/x")
                .header(header::AUTHORIZATION, auth)
                .body(Body::from("x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn read_role_cannot_put() {
    let f = fixture().await;
    let secret = marmotte_core::auth::generate_secret();
    let lookup = marmotte_core::auth::lookup_hash(&secret);
    let phc = marmotte_core::auth::hash_argon2(&secret).unwrap();
    let p = f
        .state
        .db
        .projects()
        .get_by_name(&f.project_name)
        .await
        .unwrap();
    f.state
        .db
        .api_keys()
        .create(
            p.id,
            &lookup,
            &phc,
            marmotte_core::models::Role::Read,
            Some("ro"),
        )
        .await
        .unwrap();

    let app = build_app(f.state);
    let auth = basic(&f.project_name, &secret);
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/sstate/acme/y")
                .header(header::AUTHORIZATION, auth)
                .body(Body::from("y"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn missing_entry_returns_404() {
    let f = fixture().await;
    let app = build_app(f.state);
    let auth = basic(&f.project_name, &f.api_key);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/sstate/acme/none/here")
                .header(header::AUTHORIZATION, auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn head_returns_etag_without_body() {
    let f = fixture().await;
    let app = build_app(f.state);
    let auth = basic(&f.project_name, &f.api_key);
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/sstate/acme/h")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::from("body"))
                .unwrap(),
        )
        .await
        .unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/sstate/acme/h")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get(header::ETAG).is_some());
}

#[tokio::test]
async fn put_above_threshold_triggers_gc_eventually() {
    // Use a throwaway fixture only to capture the default Config shape.
    let f0 = fixture().await;
    let mut cfg = f0.state.config.clone();
    // Set the global quota to 1 byte so any upload exceeds the threshold and
    // immediately triggers a post-PUT GC sweep.
    cfg.gc.global_quota_bytes = 1;
    let f = common::fixture_with_config(cfg).await;
    let app = build_app(f.state.clone());
    let auth = basic(&f.project_name, &f.api_key);
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/sstate/acme/big")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::from(vec![0u8; 1024]))
                .unwrap(),
        )
        .await
        .unwrap();
    // Best-effort poll: wait up to 2 s for the spawned GC to evict the blob.
    for _ in 0..40 {
        let total = f.state.db.blobs().total_size().await.unwrap();
        if total == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("post-PUT GC did not run within 2 s");
}

// Rust guideline compliant 2026-05-06
