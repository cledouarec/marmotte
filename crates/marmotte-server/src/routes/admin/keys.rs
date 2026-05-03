//! API-key endpoints.
//
// Rust guideline compliant 2026-05-06

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use marmotte_core::auth::{AdminAuth, generate_secret, hash_argon2, lookup_hash};
use serde_json::json;

use crate::{
    error::ApiError,
    routes::admin::dto::{ApiKeyCreated, ApiKeyView, CreateApiKey},
    state::AppState,
};

/// Routes for `/projects/{id}/keys` and `/projects/{id}/keys/{kid}`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/keys", post(create).get(list))
        .route("/projects/{id}/keys/{kid}", axum::routing::delete(revoke))
}

async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path(id): Path<i64>,
    Json(req): Json<CreateApiKey>,
) -> Result<(StatusCode, Json<ApiKeyCreated>), ApiError> {
    // Ensure the project exists (returns 404 if not).
    let _p = state.db.projects().get(id).await?;
    let secret = generate_secret();
    let lookup = lookup_hash(&secret);
    let phc = hash_argon2(&secret).map_err(crate::error::ApiError::from)?;
    let key = state
        .db
        .api_keys()
        .create(id, &lookup, &phc, req.role, req.label.as_deref())
        .await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "key.create",
            Some(&key.id.to_string()),
            Some(&json!({"role": req.role})),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ApiKeyCreated {
            id: key.id,
            project_id: key.project_id,
            role: key.role,
            label: key.label,
            created_at: key.created_at,
            secret,
        }),
    ))
}

async fn list(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ApiKeyView>>, ApiError> {
    let v = state.db.api_keys().list(id).await?;
    Ok(Json(v.into_iter().map(Into::into).collect()))
}

async fn revoke(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path((id, kid)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    state.db.api_keys().revoke(id, kid).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "key.revoke",
            Some(&kid.to_string()),
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
