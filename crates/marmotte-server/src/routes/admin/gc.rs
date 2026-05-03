//! GC endpoints.
//

use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::post,
};
use marmotte_core::auth::AdminAuth;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gc/run", post(run))
        .route("/gc/orphan-scan", post(orphan_scan))
}

#[derive(Debug, Deserialize)]
struct DryRun {
    #[serde(default)]
    dry_run: bool,
}

async fn run(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Query(q): Query<DryRun>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let r = state.gc.run(q.dry_run).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "gc.run",
            None,
            Some(&json!({"dry_run": q.dry_run})),
        )
        .await?;
    crate::metrics::GC_RUNS
        .with_label_values(&[if q.dry_run { "true" } else { "false" }])
        .inc();
    Ok((
        StatusCode::OK,
        Json(json!({
            "dry_run": q.dry_run,
            "evicted_entries": r.evicted_entries,
            "evicted_bytes": r.evicted_bytes,
            "freed_blobs": r.freed_blobs,
            "usage_global_bytes": r.usage_global_bytes,
        })),
    ))
}

async fn orphan_scan(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Query(q): Query<DryRun>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let r = state.gc.orphan_scan(q.dry_run).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "gc.orphan_scan",
            None,
            Some(&json!({"dry_run": q.dry_run})),
        )
        .await?;
    Ok((
        StatusCode::OK,
        Json(json!({
            "dry_run": q.dry_run,
            "disk_orphans_removed": r.disk_orphans_removed,
            "db_orphans_removed": r.db_orphans_removed,
        })),
    ))
}
