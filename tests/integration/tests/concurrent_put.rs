//! 50 concurrent PUTs of identical content end up as exactly one blob on disk
//! and one entry per logical path.
//

use futures::future::join_all;
use marmotte_integration_tests::Harness;
use reqwest::header;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_concurrent_puts_dedup_correctly() {
    let h = Harness::spawn().await.unwrap();
    let body = vec![42u8; 32 * 1024];
    let client = reqwest::Client::new();

    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let client = client.clone();
            let url = h.url.join(&format!("sstate/acme/p{i}")).unwrap();
            let auth = h.project_basic();
            let body = body.clone();
            tokio::spawn(async move {
                let r = client
                    .put(url)
                    .header(header::AUTHORIZATION, auth)
                    .body(body)
                    .send()
                    .await
                    .unwrap();
                assert!(r.status().is_success(), "{}", r.status());
            })
        })
        .collect();
    join_all(tasks).await;

    // One blob, refcount 50.
    let blobs: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT hash, refcount, size_bytes FROM blobs")
            .fetch_all(h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(blobs.len(), 1);
    assert_eq!(blobs[0].1, 50);

    // 50 entries.
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM entries")
        .fetch_one(h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(count.0, 50);

    // tmp/ is empty.
    let mut rd = tokio::fs::read_dir(h.state.config.server.storage_root.join("tmp"))
        .await
        .unwrap();
    assert!(rd.next_entry().await.unwrap().is_none(), "tmp not cleaned");
}
