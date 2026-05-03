//! Tracing + Prometheus initialization.
//
// Rust guideline compliant 2026-05-06

use marmotte_core::config::{LogFormat, LoggingConfig};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Initializes the global `tracing` subscriber. Idempotent: calling it
/// twice silently keeps the first installation.
pub fn init_tracing(cfg: &LoggingConfig) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    let registry = tracing_subscriber::registry().with(filter);

    let _ = match cfg.format {
        LogFormat::Json => registry
            .with(fmt::layer().json().flatten_event(true))
            .try_init(),
        LogFormat::Pretty => registry.with(fmt::layer().pretty()).try_init(),
    };
}

// Rust guideline compliant 2026-05-06
