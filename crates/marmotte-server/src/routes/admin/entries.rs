//! Entry endpoints: list, get one, delete, pin/unpin.
//

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use marmotte_core::{
    auth::AdminAuth,
    db::entries::{ListFilter, SortField, SortOrder},
    error::CoreError,
    models::{Entry, Kind},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/entries", get(list))
        .route(
            "/projects/{id}/entries/{eid}",
            get(get_one).delete(delete_one),
        )
        .route("/projects/{id}/entries/{eid}/pin", post(pin).delete(unpin))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    kind: Option<String>,
    pinned: Option<bool>,
    path_prefix: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    count: Option<bool>,
}

async fn list(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
    Path(id): Path<i64>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let _p = state.db.projects().get(id).await?;

    let kind = q.kind.as_deref().map(Kind::from_str).transpose()?;
    let sort = q.sort.as_deref().map(parse_sort).transpose()?;
    let order = q.order.as_deref().map(parse_order).transpose()?;

    let filter = ListFilter {
        kind,
        pinned: q.pinned,
        path_prefix: q.path_prefix,
        sort,
        order,
        limit: q.limit,
        cursor: q.cursor,
    };
    let page = state.db.entries().list(id, filter).await?;
    let entries: Vec<Value> = page.entries.iter().map(entry_to_json).collect();

    let mut body = json!({
        "entries": entries,
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
    });
    if q.count.unwrap_or(false) {
        let total: (i64,) = sqlx::query_as("SELECT count(*) FROM entries WHERE project_id = ?")
            .bind(id)
            .fetch_one(state.db.pool())
            .await?;
        body["total"] = json!(total.0);
    }
    Ok(Json(body))
}

fn parse_sort(s: &str) -> Result<SortField, CoreError> {
    Ok(match s {
        "path" => SortField::Path,
        "last_accessed" => SortField::LastAccessed,
        "size" => SortField::Size,
        "created_at" => SortField::CreatedAt,
        other => return Err(CoreError::InvalidInput(format!("sort: {other}"))),
    })
}

fn parse_order(s: &str) -> Result<SortOrder, CoreError> {
    Ok(match s {
        "asc" => SortOrder::Asc,
        "desc" => SortOrder::Desc,
        other => return Err(CoreError::InvalidInput(format!("order: {other}"))),
    })
}

fn entry_to_json(e: &Entry) -> Value {
    json!({
        "id": e.id, "kind": e.kind, "path": e.path,
        "blob_hash": e.blob_hash, "size_bytes": e.size_bytes,
        "pinned": e.pinned, "created_at": e.created_at,
        "last_accessed": e.last_accessed,
    })
}

async fn get_one(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
    Path((id, eid)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    // Resolve via list with a cursor of `(_, eid-1)` is overkill — query directly.
    let row: Option<Entry> = sqlx::query_as(
        "SELECT id, project_id, kind, path, blob_hash, size_bytes, \
                created_at, last_accessed, pinned \
         FROM entries WHERE project_id = ? AND id = ?",
    )
    .bind(id)
    .bind(eid)
    .fetch_optional(state.db.pool())
    .await?;
    let e = row.ok_or_else(|| {
        ApiError(CoreError::NotFound {
            what: format!("entry {eid}"),
        })
    })?;
    Ok(Json(entry_to_json(&e)))
}

async fn delete_one(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path((id, eid)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    state.db.entries().delete(id, eid).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "entry.delete",
            Some(&eid.to_string()),
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pin(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path((id, eid)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    state.db.entries().set_pinned(id, eid, true).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "entry.pin",
            Some(&eid.to_string()),
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unpin(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path((id, eid)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    state.db.entries().set_pinned(id, eid, false).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "entry.unpin",
            Some(&eid.to_string()),
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
