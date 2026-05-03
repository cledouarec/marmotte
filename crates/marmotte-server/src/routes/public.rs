//! Public, unauthenticated routes (`/healthz`, `/readyz`, `/metrics`).
//!
//! These three endpoints are intentionally open — they carry no sensitive data
//! and must be reachable by load-balancer probes and monitoring scrapers
//! without authentication.
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /healthz` | Liveness probe — always `200 ok` if the process is up. |
//! | `GET /readyz` | Readiness probe — checks `DB` and storage writability. |
//! | `GET /metrics` | Prometheus text-format scrape endpoint. |
//

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};

use crate::state::AppState;

/// Mounts the public route group onto a fresh [`Router`].
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
}

/// Liveness handler — always `200 ok\n` while the process runs.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

/// Readiness handler — verifies `DB` connectivity and storage writability.
///
/// Returns `503 Service Unavailable` with a descriptive body if either check
/// fails, so load balancers can route traffic away from a degraded instance.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    // DB ping: a trivial query is sufficient to confirm the pool is open.
    if sqlx::query("SELECT 1")
        .execute(state.db.pool())
        .await
        .is_err()
    {
        return (StatusCode::SERVICE_UNAVAILABLE, "db_unavailable\n");
    }

    // Storage write probe: create a sentinel file in the tmp directory and
    // immediately delete it to confirm the store is writable.
    let probe = state.store.root().join("tmp").join(".readyz");
    if tokio::fs::write(&probe, b"r").await.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "storage_unwritable\n");
    }
    let _ = tokio::fs::remove_file(&probe).await;

    (StatusCode::OK, "ready\n")
}

/// Prometheus text-format scrape handler.
///
/// Encodes the default registry and returns the result with a `200 OK`.
/// Returns `500 Internal Server Error` if encoding fails (which should never
/// happen under normal circumstances).
async fn metrics() -> impl IntoResponse {
    use prometheus::Encoder as _;

    let metrics = prometheus::default_registry().gather();
    let mut buf = Vec::with_capacity(4096);
    if prometheus::TextEncoder::new()
        .encode(&metrics, &mut buf)
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, Vec::new());
    }
    (StatusCode::OK, buf)
}
