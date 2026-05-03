//! Cursor pagination: stable across full traversal, tolerates filters.
//
// Rust guideline compliant 2026-05-06

use marmotte_integration_tests::Harness;
use reqwest::header;
use serde_json::Value;

#[tokio::test]
async fn pagination_traverses_all_entries_exactly_once() {
    let h = Harness::spawn().await.unwrap();
    let pid = h
        .state
        .db
        .projects()
        .get_by_name(&h.project)
        .await
        .unwrap()
        .id;
    let client = reqwest::Client::new();
    for i in 0..25 {
        client
            .put(h.url.join(&format!("sstate/acme/p/{i:02}")).unwrap())
            .header(header::AUTHORIZATION, h.project_basic())
            .body(format!("body-{i}"))
            .send()
            .await
            .unwrap();
    }

    let mut seen = std::collections::HashSet::<String>::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = h
            .url
            .join(&format!(
                "api/v1/admin/projects/{pid}/entries?limit=7&sort=path&order=asc"
            ))
            .unwrap();
        if let Some(c) = &cursor {
            url.query_pairs_mut().append_pair("cursor", c);
        }
        let r = client
            .get(url)
            .header(header::AUTHORIZATION, h.admin_bearer())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let v: Value = r.json().await.unwrap();
        for e in v["entries"].as_array().unwrap() {
            let p = e["path"].as_str().unwrap().to_string();
            assert!(seen.insert(p), "duplicate page entry");
        }
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 25);
}
