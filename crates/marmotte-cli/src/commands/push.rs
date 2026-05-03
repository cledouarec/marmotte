//! `marmotte push`: upload a directory tree as cache entries.
//

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures::{StreamExt, stream::FuturesUnordered};
use reqwest::{Client, StatusCode, header};
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncReadExt, sync::Semaphore};

use crate::error::{CliError, CliResult};

/// HTTP status codes that warrant a retry with exponential backoff.
///
/// `5xx` gateway/server errors are transient; client errors (4xx) are not.
const RETRYABLE_STATUSES: &[StatusCode] = &[
    StatusCode::INTERNAL_SERVER_ERROR,
    StatusCode::BAD_GATEWAY,
    StatusCode::SERVICE_UNAVAILABLE,
    StatusCode::GATEWAY_TIMEOUT,
];

/// Maximum number of upload retries before giving up on a single file.
const MAX_RETRIES: u32 = 5;

/// Initial backoff in milliseconds; doubles each retry attempt.
const INITIAL_BACKOFF_MS: u64 = 200;

/// Recursively pushes files in `dir` to the cache.
///
/// HEAD-checks each file using `If-None-Match` against its `SHA-256` etag;
/// skips if the server already has an identical copy. Otherwise `PUT`s the
/// body with exponential backoff on transient `5xx` errors. Exits with a
/// non-zero status code if any file fails after all retries.
///
/// # Errors
///
/// Returns an error if the `HTTP` client cannot be built or directory walking
/// fails. Per-file errors are counted and cause a process exit with code 1.
pub async fn run(
    project: String,
    kind: String,
    base_url: url::Url,
    api_key: String,
    concurrency: usize,
    dry_run: bool,
    dir: PathBuf,
) -> CliResult<()> {
    let client = Client::builder()
        // 10 s connect timeout: avoids hanging on unreachable hosts.
        .connect_timeout(Duration::from_secs(10))
        // 15 min request timeout: covers large Yocto artifacts on slow links.
        .timeout(Duration::from_mins(15))
        .build()?;

    let mut files = vec![];
    walk(&dir, &dir, &mut files)?;
    let total = files.len();
    let sem = Arc::new(Semaphore::new(concurrency));

    let auth_header = Arc::new(format!(
        "Basic {}",
        STANDARD.encode(format!("{project}:{api_key}"))
    ));

    let mut tasks = FuturesUnordered::new();
    for (rel, abs) in files {
        let permit = sem.clone().acquire_owned().await.unwrap();
        let url = base_url.join(&format!("{kind}/{project}/{}", rel.to_string_lossy()))?;
        let client = client.clone();
        let auth = Arc::clone(&auth_header);
        tasks.push(tokio::spawn(async move {
            let _p = permit;
            Box::pin(push_one(&client, url, &abs, &auth, dry_run)).await
        }));
    }

    let mut pushed = 0u64;
    let mut skipped = 0u64;
    let mut failed = 0u64;
    while let Some(res) = tasks.next().await {
        // Collapse `Result<CliResult<Outcome>, JoinError>` into `CliResult<Outcome>`
        // by lifting the join failure through the `#[from]` conversion.
        match res.map_err(CliError::from).and_then(|inner| inner) {
            Ok(Outcome::Pushed) => pushed += 1,
            Ok(Outcome::Skipped) => skipped += 1,
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    name: "push.file.failed",
                    error = %e,
                    "file push failed: {{error}}",
                );
            }
        }
    }

    println!("total {total} | pushed {pushed} | skipped {skipped} | failed {failed}");
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Recursively collects `(relative, absolute)` file pairs from `dir`.
fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, PathBuf)>) -> CliResult<()> {
    for ent in std::fs::read_dir(dir)? {
        let ent = ent?;
        let path = ent.path();
        if ent.file_type()?.is_dir() {
            walk(root, &path, out)?;
        } else if ent.file_type()?.is_file() {
            let rel = path.strip_prefix(root)?.to_path_buf();
            out.push((rel, path));
        }
    }
    Ok(())
}

/// Result of attempting to push a single file.
enum Outcome {
    /// File was uploaded (or would have been in dry-run mode).
    Pushed,
    /// Server already has an identical copy; upload skipped.
    Skipped,
}

/// Pushes a single file: `HEAD` with `If-None-Match`, then `PUT` with backoff.
async fn push_one(
    client: &Client,
    url: url::Url,
    abs: &Path,
    auth: &str,
    dry_run: bool,
) -> CliResult<Outcome> {
    let sha = Box::pin(sha256_file(abs)).await?;
    let etag = format!("\"{sha}\"");

    // HEAD with If-None-Match: skip if server etag matches.
    let resp = client
        .head(url.clone())
        .header(header::AUTHORIZATION, auth)
        .header(header::IF_NONE_MATCH, &etag)
        .send()
        .await?;
    match resp.status() {
        StatusCode::NOT_MODIFIED => return Ok(Outcome::Skipped),
        StatusCode::OK => {
            if resp
                .headers()
                .get(header::ETAG)
                .and_then(|v| v.to_str().ok())
                == Some(etag.as_str())
            {
                return Ok(Outcome::Skipped);
            }
        }
        StatusCode::NOT_FOUND => {}
        s if s.is_success() => {}
        other => return Err(CliError::Other(format!("HEAD returned {other}"))),
    }

    if dry_run {
        return Ok(Outcome::Pushed);
    }

    // PUT body with exponential backoff on transient 5xx errors.
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let body = fs::read(abs).await?;
        let resp = client
            .put(url.clone())
            .header(header::AUTHORIZATION, auth)
            .body(Bytes::from(body))
            .send()
            .await?;
        if resp.status().is_success() {
            return Ok(Outcome::Pushed);
        }
        if RETRYABLE_STATUSES.contains(&resp.status()) && attempt < MAX_RETRIES {
            tokio::time::sleep(Duration::from_millis(
                INITIAL_BACKOFF_MS * 2u64.pow(attempt),
            ))
            .await;
            continue;
        }
        return Err(CliError::Other(format!("PUT {url}: {}", resp.status())));
    }
}

/// Computes the `SHA-256` hex digest of a file's contents.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read.
async fn sha256_file(p: &Path) -> CliResult<String> {
    let mut f = tokio::fs::File::open(p).await?;
    let mut h = Sha256::new();
    // 64 KiB heap buffer: amortizes syscall overhead without large stack frames.
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}
