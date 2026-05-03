//! Stats + audit endpoints.
//
// Rust guideline compliant 2026-05-06

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use marmotte_core::auth::AdminAuth;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/stats", get(global_stats))
        .route("/stats/projects/{id}", get(project_stats))
        .route("/audit", get(audit))
}

async fn global_stats(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
) -> Result<Json<Value>, ApiError> {
    let total = state.db.blobs().total_size().await?;
    let projects = state.db.projects().list().await?;
    Ok(Json(json!({
        "global_bytes": total,
        "projects": projects.len(),
    })))
}

async fn project_stats(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let p = state.db.projects().get(id).await?;
    let usage = state.db.entries().project_usage(id).await?;
    let (hits, hits_b) = state.db.stats().sum(Some(id), "hit", 0).await?;
    let (puts, puts_b) = state.db.stats().sum(Some(id), "put", 0).await?;
    Ok(Json(json!({
        "project": { "id": p.id, "name": p.name, "quota_bytes": p.quota_bytes },
        "usage_bytes": usage,
        "hits": hits, "hits_bytes": hits_b,
        "puts": puts, "puts_bytes": puts_b,
    })))
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

async fn audit(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Value>, ApiError> {
    let rows = state
        .db
        .stats()
        .audit_since(q.since.unwrap_or(0), q.limit.unwrap_or(100).clamp(1, 1000))
        .await?;
    Ok(Json(json!({ "entries": rows })))
}
