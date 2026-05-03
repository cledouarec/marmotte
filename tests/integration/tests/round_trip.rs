//! Full PUT → GET → HEAD → DELETE cycle.
//
// Rust guideline compliant 2026-05-06

use marmotte_integration_tests::Harness;
use reqwest::{StatusCode, header};

#[tokio::test]
async fn put_get_head_delete_cycle() {
    let h = Harness::spawn().await.unwrap();
    let body = b"some sstate artifact" as &[u8];
    let url = h.url.join("sstate/acme/foo.tar.zst").unwrap();
    let client = reqwest::Client::new();

    let r = client
        .put(url.clone())
        .header(header::AUTHORIZATION, h.project_basic())
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    let etag = r.headers().get(header::ETAG).cloned().unwrap();

    let r = client
        .get(url.clone())
        .header(header::AUTHORIZATION, h.project_basic())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get(header::ETAG).unwrap(), &etag);
    assert_eq!(r.bytes().await.unwrap().as_ref(), body);

    let r = client
        .head(url.clone())
        .header(header::AUTHORIZATION, h.project_basic())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers().get(header::ETAG).unwrap(), &etag);
    // Reqwest parses Content-Length from the response header.
    let cl: u64 = r
        .headers()
        .get(header::CONTENT_LENGTH)
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(cl, body.len() as u64);

    // Delete via admin API.
    let pid = h
        .state
        .db
        .projects()
        .get_by_name(&h.project)
        .await
        .unwrap()
        .id;
    let entries = h
        .state
        .db
        .entries()
        .list(pid, marmotte_core::db::entries::ListFilter::default())
        .await
        .unwrap();
    let eid = entries.entries[0].id;
    let admin_url = h
        .url
        .join(&format!("api/v1/admin/projects/{pid}/entries/{eid}"))
        .unwrap();
    let r = client
        .delete(admin_url)
        .header(header::AUTHORIZATION, h.admin_bearer())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);

    let r = client
        .get(url)
        .header(header::AUTHORIZATION, h.project_basic())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
