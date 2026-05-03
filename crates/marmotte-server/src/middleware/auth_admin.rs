//! Admin Bearer-token middleware.
//
// Rust guideline compliant 2026-05-06

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use marmotte_core::error::CoreError;

use crate::state::AppState;

/// Middleware: extracts a Bearer token, verifies it, and injects [`AdminAuth`]
/// into request extensions.
///
/// [`AdminAuth`]: marmotte_core::auth::AdminAuth
pub async fn middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(token) = parse_bearer(&headers) else {
        return unauthorized();
    };
    match state.auth.authenticate_admin(&token).await {
        Ok(a) => {
            req.extensions_mut().insert(a);
            next.run(req).await
        }
        Err(CoreError::Unauthorized) => unauthorized(),
        Err(e) => crate::error::ApiError(e).into_response(),
    }
}

fn parse_bearer(h: &HeaderMap) -> Option<String> {
    let raw = h.get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(str::to_string)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response()
}
