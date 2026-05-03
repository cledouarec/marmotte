//! Auth boundaries: cross-project rejection, role enforcement, missing creds.
//
// Rust guideline compliant 2026-05-06

use base64::{Engine, engine::general_purpose::STANDARD};
use marmotte_integration_tests::Harness;
use reqwest::{StatusCode, header};

#[tokio::test]
async fn cross_project_url_returns_403() {
    let h = Harness::spawn().await.unwrap();
    let url = h.url.join("sstate/other/foo").unwrap();
    let r = reqwest::Client::new()
        .put(url)
        .header(header::AUTHORIZATION, h.project_basic())
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn missing_auth_returns_401() {
    let h = Harness::spawn().await.unwrap();
    let r = reqwest::get(h.url.join("sstate/acme/foo").unwrap())
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn read_role_cannot_put() {
    let h = Harness::spawn().await.unwrap();
    let p = h.state.db.projects().get_by_name(&h.project).await.unwrap();
    let secret = marmotte_core::auth::generate_secret();
    h.state
        .db
        .api_keys()
        .create(
            p.id,
            &marmotte_core::auth::lookup_hash(&secret),
            &marmotte_core::auth::hash_argon2(&secret).unwrap(),
            marmotte_core::models::Role::Read,
            None,
        )
        .await
        .unwrap();
    let auth = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", h.project, secret))
    );
    let r = reqwest::Client::new()
        .put(h.url.join("sstate/acme/x").unwrap())
        .header(header::AUTHORIZATION, auth)
        .body("y")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}
