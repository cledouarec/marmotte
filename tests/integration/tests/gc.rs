//! End-to-end GC: project quota eviction with pin protection.
//
// Rust guideline compliant 2026-05-06

use marmotte_integration_tests::Harness;
use reqwest::{StatusCode, header};

#[tokio::test]
async fn manual_gc_evicts_lru_keeps_pinned() {
    let h = Harness::spawn().await.unwrap();
    let pid = h
        .state
        .db
        .projects()
        .get_by_name(&h.project)
        .await
        .unwrap()
        .id;
    // Put project quota at 100 bytes.
    h.state
        .db
        .projects()
        .update(pid, None, Some(Some(100)), None)
        .await
        .unwrap();

    let client = reqwest::Client::new();
    for i in 0u8..6 {
        let body = vec![b'a' + i; 50];
        client
            .put(h.url.join(&format!("sstate/acme/p{i}")).unwrap())
            .header(header::AUTHORIZATION, h.project_basic())
            .body(body)
            .send()
            .await
            .unwrap();
    }

    // Pin entry 0 (the oldest).
    let entries = h
        .state
        .db
        .entries()
        .list(pid, marmotte_core::db::entries::ListFilter::default())
        .await
        .unwrap();
    let oldest = entries.entries.iter().min_by_key(|e| e.id).unwrap().clone();
    client
        .post(
            h.url
                .join(&format!(
                    "api/v1/admin/projects/{pid}/entries/{}/pin",
                    oldest.id
                ))
                .unwrap(),
        )
        .header(header::AUTHORIZATION, h.admin_bearer())
        .send()
        .await
        .unwrap();

    // Manual GC.
    let r = client
        .post(h.url.join("api/v1/admin/gc/run").unwrap())
        .header(header::AUTHORIZATION, h.admin_bearer())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // Pinned survived.
    assert!(
        h.state
            .db
            .entries()
            .find(pid, marmotte_core::models::Kind::Sstate, &oldest.path)
            .await
            .unwrap()
            .is_some()
    );
    // Project usage now ≤ quota (100 bytes).
    let usage = h.state.db.entries().project_usage(pid).await.unwrap();
    assert!(usage <= 100, "usage {usage}");
}
