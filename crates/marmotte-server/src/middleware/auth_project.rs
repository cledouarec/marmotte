//! Project Basic-auth middleware. On success, attaches `ProjectAuth` to the
//! request extensions and continues; on failure, returns 401.
//
// Rust guideline compliant 2026-05-06

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use marmotte_core::error::CoreError;

use crate::state::AppState;

/// Middleware: extracts and verifies HTTP Basic credentials, then injects
/// [`ProjectAuth`] into request extensions.
pub async fn middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(creds) = parse_basic(&headers) else {
        return unauthorized();
    };

    match state
        .auth
        .authenticate_project(&creds.user, &creds.pass)
        .await
    {
        Ok(auth) => {
            req.extensions_mut().insert(auth);
            next.run(req).await
        }
        Err(CoreError::Unauthorized) => unauthorized(),
        Err(e) => crate::error::ApiError(e).into_response(),
    }
}

#[derive(Debug)]
struct Basic {
    user: String,
    pass: String,
}

fn parse_basic(h: &HeaderMap) -> Option<Basic> {
    let raw = h.get(header::AUTHORIZATION)?.to_str().ok()?;
    let b64 = raw.strip_prefix("Basic ")?;
    let decoded = STANDARD.decode(b64).ok()?;
    let s = std::str::from_utf8(&decoded).ok()?;
    let (u, p) = s.split_once(':')?;
    Some(Basic {
        user: u.into(),
        pass: p.into(),
    })
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"marmotte\"")],
        "unauthorized\n",
    )
        .into_response()
}
