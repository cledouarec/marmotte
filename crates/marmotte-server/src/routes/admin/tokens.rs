//! Admin-token endpoints.
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
    routes::admin::dto::{AdminTokenCreated, AdminTokenView, CreateAdminToken},
    state::AppState,
};

/// Routes for `/tokens` and `/tokens/{id}`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tokens", post(create).get(list))
        .route("/tokens/{id}", axum::routing::delete(revoke))
}

async fn create(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Json(req): Json<CreateAdminToken>,
) -> Result<(StatusCode, Json<AdminTokenCreated>), ApiError> {
    let secret = generate_secret();
    let lookup = lookup_hash(&secret);
    let phc = hash_argon2(&secret)?;
    let t = state
        .db
        .admin_tokens()
        .create(&lookup, &phc, req.label.as_deref())
        .await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "token.create",
            Some(&t.id.to_string()),
            Some(&json!({"label": t.label})),
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(AdminTokenCreated {
            id: t.id,
            label: t.label,
            created_at: t.created_at,
            secret,
        }),
    ))
}

async fn list(
    State(state): State<AppState>,
    Extension(_a): Extension<AdminAuth>,
) -> Result<Json<Vec<AdminTokenView>>, ApiError> {
    let v = state.db.admin_tokens().list().await?;
    Ok(Json(
        v.into_iter()
            .map(|t| AdminTokenView {
                id: t.id,
                label: t.label,
                created_at: t.created_at,
                revoked_at: t.revoked_at,
            })
            .collect(),
    ))
}

async fn revoke(
    State(state): State<AppState>,
    Extension(auth): Extension<AdminAuth>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state.db.admin_tokens().revoke(id).await?;
    state
        .db
        .stats()
        .audit(
            auth.token_label.as_deref(),
            "token.revoke",
            Some(&id.to_string()),
            None,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
