//! HTTP layer for Marmotte.
//
// Rust guideline compliant 2026-05-06

/// HTTP error type with RFC 7807 problem+json rendering.
pub mod error;
/// Prometheus collectors — counters, gauges, and histograms.
pub mod metrics;
/// Tower middleware for request authentication.
pub mod middleware;
/// Tracing and Prometheus initialization helpers.
pub mod observability;
/// HTTP route groups, one submodule per logical group.
pub mod routes;
/// Shared application state passed to axum handlers.
pub mod state;

use std::time::Duration;

use axum::{Router, http::StatusCode};
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer, trace::TraceLayer};

pub use state::{AppState, AppStateInner};

/// Spawns a background task that runs GC on a fixed interval.
///
/// The ticker fires immediately on first tick (tokio default behavior) and then
/// repeats every `config.gc.interval_secs` seconds. Missed ticks are skipped
/// rather than piled up, so a slow GC run does not cause a backlog.
///
/// Errors from [`marmotte_core::gc::GcSvc::run`] are logged at `WARN` level
/// and do not stop the ticker loop.
fn spawn_gc_ticker(state: &AppState) {
    let interval = Duration::from_secs(state.config.gc.interval_secs);
    let gc = state.gc.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip rather than pile up missed ticks if a GC run takes longer than
        // the configured interval.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(e) = gc.run(false).await {
                tracing::warn!(
                    name: "gc.periodic.failed",
                    error = %e,
                    "periodic GC failed: {{error}}",
                );
            }
        }
    });
}

/// Builds the top-level [`axum::Router`] with global middleware applied.
///
/// Routes will be merged in by later tasks.
pub fn build_app(state: AppState) -> Router {
    let timeout = Duration::from_secs(state.config.server.request_timeout_secs);
    // Cast is intentional: upload_max_bytes is a byte count bounded well
    // below usize::MAX on all supported 32- and 64-bit targets.
    #[allow(clippy::cast_possible_truncation)]
    let upload_limit = state.config.server.upload_max_bytes as usize;

    metrics::init();
    spawn_gc_ticker(&state);

    Router::new()
        .merge(routes::public::router())
        .merge(routes::yocto::router(state.clone()))
        .merge(routes::admin::router(state.clone()))
        .with_state(state)
        .layer(RequestBodyLimitLayer::new(upload_limit))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .layer(TraceLayer::new_for_http())
}
