//! Yocto-facing cache routes (`/sstate/...`, `/downloads/...`).
//

use axum::{
    Extension, Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::from_fn_with_state,
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::StreamExt as _;
use marmotte_core::{auth::ProjectAuth, error::CoreError, models::Kind};
use tokio_util::io::ReaderStream;

use crate::{error::ApiError, middleware::auth_project, state::AppState};

/// Mounts the Yocto-facing routes.
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/sstate/{project}/{*path}",
            get(get_sstate).head(head_sstate).put(put_sstate),
        )
        .route(
            "/downloads/{project}/{filename}",
            get(get_downloads).head(head_downloads).put(put_downloads),
        )
        .layer(from_fn_with_state(state, auth_project::middleware))
}

/// Verifies that the URL project segment matches the authenticated project name.
fn ensure_project(auth: &ProjectAuth, url_project: &str) -> Result<(), ApiError> {
    if auth.project_name != url_project {
        return Err(ApiError(CoreError::Forbidden {
            reason: "url project does not match credential".into(),
        }));
    }
    Ok(())
}

/// Core read logic shared by GET and HEAD handlers.
///
/// When `head_only` is `true` the body is omitted and only headers are returned.
async fn get_one(
    state: AppState,
    auth: ProjectAuth,
    kind: Kind,
    project_in_url: &str,
    path: &str,
    head_only: bool,
) -> Result<Response, ApiError> {
    ensure_project(&auth, project_in_url)?;
    if !auth.role.allows_read() {
        return Err(ApiError(CoreError::Forbidden {
            reason: "read".into(),
        }));
    }

    let entry = state
        .db
        .entries()
        .find(auth.project_id, kind, path)
        .await?
        .ok_or_else(|| {
            ApiError(CoreError::NotFound {
                what: format!("{} {path}", kind.as_str()),
            })
        })?;

    let mut headers = HeaderMap::new();
    // `size_bytes` is a decimal integer — always valid ASCII.
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&entry.size_bytes.to_string())
            .expect("decimal integer is valid header value"),
    );
    // SHA-256 hex is 64 ASCII characters — safe to use in a header value.
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", entry.blob_hash))
            .expect("sha256 hex is valid ASCII"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        // max-age=31536000 = 1 year; immutable signals the blob never changes
        // for a given hash, so clients and CDNs can cache indefinitely.
        HeaderValue::from_static("public, immutable, max-age=31536000"),
    );

    if head_only {
        return Ok((StatusCode::OK, headers).into_response());
    }

    let file = state.store.read_blob(&entry.blob_hash).await?;
    let stream = ReaderStream::new(file);

    // Fire-and-forget last_accessed bump — errors are logged inside `touch`.
    // Clone the `Db` handle so the spawned task owns it independently of the
    // handler's `state` binding, satisfying the `'static` bound on `spawn`.
    let db = state.db.clone();
    let id = entry.id;
    tokio::spawn(async move {
        db.entries().touch(id).await;
    });

    crate::metrics::CACHE_HITS
        .with_label_values(&[kind.as_str(), &auth.project_name])
        .inc();

    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

/// Emits a 404 miss response and increments the cache-miss counter.
fn miss_response(state: &AppState, auth: &ProjectAuth, kind: Kind) -> Response {
    crate::metrics::CACHE_MISSES
        .with_label_values(&[kind.as_str(), &auth.project_name])
        .inc();
    // `state` is retained here for potential future per-request audit use.
    let _ = state;
    (StatusCode::NOT_FOUND, "not_found\n").into_response()
}

async fn get_sstate(
    State(state): State<AppState>,
    Extension(auth): Extension<ProjectAuth>,
    Path((project, path)): Path<(String, String)>,
) -> Response {
    match get_one(
        state.clone(),
        auth.clone(),
        Kind::Sstate,
        &project,
        &path,
        false,
    )
    .await
    {
        Ok(r) => r,
        Err(ApiError(CoreError::NotFound { .. })) => miss_response(&state, &auth, Kind::Sstate),
        Err(e) => e.into_response(),
    }
}

async fn head_sstate(
    State(state): State<AppState>,
    Extension(auth): Extension<ProjectAuth>,
    Path((project, path)): Path<(String, String)>,
) -> Response {
    match get_one(
        state.clone(),
        auth.clone(),
        Kind::Sstate,
        &project,
        &path,
        true,
    )
    .await
    {
        Ok(r) => r,
        Err(ApiError(CoreError::NotFound { .. })) => miss_response(&state, &auth, Kind::Sstate),
        Err(e) => e.into_response(),
    }
}

async fn get_downloads(
    State(state): State<AppState>,
    Extension(auth): Extension<ProjectAuth>,
    Path((project, filename)): Path<(String, String)>,
) -> Response {
    match get_one(
        state.clone(),
        auth.clone(),
        Kind::Downloads,
        &project,
        &filename,
        false,
    )
    .await
    {
        Ok(r) => r,
        Err(ApiError(CoreError::NotFound { .. })) => miss_response(&state, &auth, Kind::Downloads),
        Err(e) => e.into_response(),
    }
}

async fn head_downloads(
    State(state): State<AppState>,
    Extension(auth): Extension<ProjectAuth>,
    Path((project, filename)): Path<(String, String)>,
) -> Response {
    match get_one(
        state.clone(),
        auth.clone(),
        Kind::Downloads,
        &project,
        &filename,
        true,
    )
    .await
    {
        Ok(r) => r,
        Err(ApiError(CoreError::NotFound { .. })) => miss_response(&state, &auth, Kind::Downloads),
        Err(e) => e.into_response(),
    }
}

async fn put_sstate(
    State(state): State<AppState>,
    Extension(auth): Extension<ProjectAuth>,
    Path((project, path)): Path<(String, String)>,
    body: Body,
) -> Result<Response, ApiError> {
    do_put(state, auth, Kind::Sstate, &project, &path, body).await
}

async fn put_downloads(
    State(state): State<AppState>,
    Extension(auth): Extension<ProjectAuth>,
    Path((project, filename)): Path<(String, String)>,
    body: Body,
) -> Result<Response, ApiError> {
    do_put(state, auth, Kind::Downloads, &project, &filename, body).await
}

/// Core write logic shared by PUT handlers for both cache kinds.
async fn do_put(
    state: AppState,
    auth: ProjectAuth,
    kind: Kind,
    project_in_url: &str,
    path: &str,
    body: Body,
) -> Result<Response, ApiError> {
    ensure_project(&auth, project_in_url)?;
    if !auth.role.allows_write() {
        return Err(ApiError(CoreError::Forbidden {
            reason: "write".into(),
        }));
    }

    // `axum::Error` does not implement `Into<std::io::Error>` directly,
    // so we adapt the stream before passing it to `write_streamed`.
    let stream = body
        .into_data_stream()
        .map(|r| r.map_err(|e| std::io::Error::other(e.into_inner())));
    let written = state.store.write_streamed(stream).await?;
    let _entry = state
        .db
        .entries()
        .upsert(
            auth.project_id,
            kind,
            path,
            &written.hash,
            written.size_bytes,
        )
        .await?;

    state
        .db
        .stats()
        .bump(Some(auth.project_id), "put", 1, written.size_bytes)
        .await?;

    // Soft trigger: if global usage exceeds the configured threshold, spawn a
    // background GC sweep so quota is reclaimed without blocking the response.
    let threshold =
        state.config.gc.global_quota_bytes * u64::from(state.config.gc.trigger_threshold_pct) / 100;
    // `total_size` is a SUM of non-negative i64 values; clamp to 0 on underflow.
    let usage = u64::try_from(state.db.blobs().total_size().await?).unwrap_or(0);
    if usage > threshold {
        let gc = state.gc.clone();
        tokio::spawn(async move {
            if let Err(e) = gc.run(false).await {
                tracing::warn!(
                    name: "gc.post_put.failed",
                    error = %e,
                    "post-PUT GC failed: {{error}}",
                );
            }
        });
    }

    let mut headers = HeaderMap::new();
    // SHA-256 hex is 64 ASCII characters — safe to use in a header value.
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", written.hash)).expect("sha256 hex is valid ASCII"),
    );
    Ok((StatusCode::CREATED, headers).into_response())
}
