//! Admin API (`/api/v1/admin/*`).
//

pub mod dto;
mod entries;
mod gc;
mod keys;
mod projects;
mod stats;
mod tokens;

use axum::{Router, middleware::from_fn_with_state};

use crate::{middleware::auth_admin, state::AppState};

/// Mounts every admin route under `/api/v1/admin`.
pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .nest(
            "/api/v1/admin",
            projects::router()
                .merge(keys::router())
                .merge(tokens::router())
                .merge(entries::router())
                .merge(gc::router())
                .merge(stats::router()),
        )
        .layer(from_fn_with_state(state, auth_admin::middleware))
}
