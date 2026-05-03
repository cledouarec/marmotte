//! Prometheus collectors.
//!
//! All metrics live in the default registry. The `/metrics` route encodes
//! that registry and returns the Prometheus text exposition format.
//!
//! # Initialization
//!
//! Call [`init`] once at application startup (inside [`crate::build_app`]) to
//! force-initialize every [`std::sync::LazyLock`] static so that the `/metrics`
//! endpoint reports all families even before the first request arrives.
//! Subsequent calls to [`init`] are safe and idempotent because
//! `LazyLock::force` only executes the initializer once.
//
// Rust guideline compliant 2026-05-06

use std::sync::LazyLock;

use prometheus::{
    HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, register_histogram_vec,
    register_int_counter_vec, register_int_gauge, register_int_gauge_vec,
};

/// Total `HTTP` requests, labeled by method, route, and status.
pub static HTTP_REQUESTS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "marmotte_http_requests_total",
        "Total HTTP requests",
        &["method", "route", "status"]
    )
    .expect("register marmotte_http_requests_total")
});

/// `HTTP` request duration in seconds, labeled by route.
pub static HTTP_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "marmotte_http_request_duration_seconds",
        "HTTP request latency",
        &["route"]
    )
    .expect("register marmotte_http_request_duration_seconds")
});

/// Per-project logical storage bytes, labeled by project slug.
pub static STORAGE_BYTES_BY_PROJECT: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "marmotte_storage_bytes",
        "Logical bytes stored, per project",
        &["project"]
    )
    .expect("register marmotte_storage_bytes")
});

/// Total physical blob bytes currently on disk.
pub static STORAGE_BYTES_GLOBAL: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "marmotte_storage_bytes_global",
        "Physical bytes stored across all blobs"
    )
    .expect("register marmotte_storage_bytes_global")
});

/// Total blob row count.
pub static STORAGE_BLOBS: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!("marmotte_storage_blobs", "Total blob rows")
        .expect("register marmotte_storage_blobs")
});

/// Cache hit counter labeled by cache kind and project.
pub static CACHE_HITS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "marmotte_cache_hits_total",
        "Cache hits",
        &["kind", "project"]
    )
    .expect("register marmotte_cache_hits_total")
});

/// Cache miss counter labeled by cache kind and project.
pub static CACHE_MISSES: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "marmotte_cache_misses_total",
        "Cache misses",
        &["kind", "project"]
    )
    .expect("register marmotte_cache_misses_total")
});

/// `GC` run counter labeled by dry-run flag.
pub static GC_RUNS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!("marmotte_gc_runs_total", "GC runs", &["dry_run"])
        .expect("register marmotte_gc_runs_total")
});

/// Cumulative bytes evicted by `GC`, labeled by sweep phase.
pub static GC_EVICTED_BYTES: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "marmotte_gc_evicted_bytes_total",
        "Bytes evicted",
        &["phase"]
    )
    .expect("register marmotte_gc_evicted_bytes_total")
});

/// Cumulative entries evicted by `GC`, labeled by sweep phase.
pub static GC_EVICTED_ENTRIES: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "marmotte_gc_evicted_entries_total",
        "Entries evicted",
        &["phase"]
    )
    .expect("register marmotte_gc_evicted_entries_total")
});

/// Forces lazy initialization of every collector in the default registry.
///
/// Call this once at application startup so that all metric families appear
/// on the `/metrics` endpoint even before the first request is handled.
/// Multiple calls are safe — each `LazyLock` initializes at most once.
///
/// Vec metrics are pre-warmed with a placeholder label set so the Prometheus
/// text encoder includes their `# HELP` and `# TYPE` headers on the first
/// scrape. The placeholder values use the empty string `""` and are visible
/// in the scrape output with a counter value of `0`; this is valid Prometheus
/// exposition format.
pub fn init() {
    LazyLock::force(&HTTP_REQUESTS);
    LazyLock::force(&HTTP_DURATION);
    LazyLock::force(&STORAGE_BYTES_BY_PROJECT);
    LazyLock::force(&STORAGE_BYTES_GLOBAL);
    LazyLock::force(&STORAGE_BLOBS);
    LazyLock::force(&CACHE_HITS);
    LazyLock::force(&CACHE_MISSES);
    LazyLock::force(&GC_RUNS);
    LazyLock::force(&GC_EVICTED_BYTES);
    LazyLock::force(&GC_EVICTED_ENTRIES);

    // Pre-warm vec metrics so the Prometheus registry does not prune them as
    // empty families before the first real observation arrives. The default
    // registry's gather() skips families with zero label sets; touching each
    // family once ensures # HELP / # TYPE headers appear on the first scrape.
    //
    // Placeholder labels use empty strings ("") which are valid label values
    // in the Prometheus data model. The resulting zero-value counters are
    // harmless and will be superseded by real observations once traffic flows.
    HTTP_REQUESTS.with_label_values(&["", "", ""]).reset();
    HTTP_DURATION.with_label_values(&[""]);
    STORAGE_BYTES_BY_PROJECT.with_label_values(&[""]);
    CACHE_HITS.with_label_values(&["", ""]);
    CACHE_MISSES.with_label_values(&["", ""]);
    GC_RUNS.with_label_values(&[""]);
    GC_EVICTED_BYTES.with_label_values(&[""]);
    GC_EVICTED_ENTRIES.with_label_values(&[""]);
}
