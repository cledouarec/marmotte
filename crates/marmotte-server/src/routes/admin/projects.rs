//! Project CRUD endpoints.
//

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use marmotte_core::auth::AdminAuth;
use serde_json::json;

use crate::{
    error::ApiError,
    routes::admin::dto::{CreateProject, ProjectView, UpdateProject},
    state::AppState,
};

/// Routes for `/projects` and `/projects/{id}`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", post(create).get(list))
        .route(
            "/projects/{id}",
            get(get_one).patch(update).delete(delete_one),
        )
}

async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Json(req): Json<CreateProject>,
) -> Result<(StatusCode, Json<ProjectView>), ApiError> {
    let p = state
        .db
        .projects()
        .create(&req.name, req.quota_bytes, req.ttl_seconds)
        .await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "project.create",
            Some(&p.id.to_string()),
            Some(&json!({"name": p.name})),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(p.into())))
}

async fn list(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
) -> Result<Json<Vec<ProjectView>>, ApiError> {
    let v = state.db.projects().list().await?;
    Ok(Json(v.into_iter().map(Into::into).collect()))
}

async fn get_one(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
    Path(id): Path<i64>,
) -> Result<Json<ProjectView>, ApiError> {
    let p = state.db.projects().get(id).await?;
    Ok(Json(p.into()))
}

async fn update(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateProject>,
) -> Result<Json<ProjectView>, ApiError> {
    let p = state
        .db
        .projects()
        .update(id, req.name.as_deref(), req.quota_bytes, req.ttl_seconds)
        .await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "project.update",
            Some(&id.to_string()),
            None,
        )
        .await?;
    Ok(Json(p.into()))
}

async fn delete_one(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.db.projects().delete(id).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "project.delete",
            Some(&id.to_string()),
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
