//! Canonical error type for the `marmotte` CLI.
//!
//! Aggregates errors raised while running CLI subcommands: core service
//! failures, filesystem I/O, HTTP transport, URL parsing, path manipulation,
//! and tokio task join failures. Higher layers render the resulting error via
//! `Debug` (the `tokio::main` `Termination` impl).
//

use thiserror::Error;

/// Errors emitted by `marmotte-cli` subcommands.
#[derive(Debug, Error)]
pub enum CliError {
    /// Failure originating from `marmotte-core` (config, db, storage, auth).
    #[error(transparent)]
    Core(#[from] marmotte_core::error::CoreError),

    /// Filesystem or socket I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP transport failure (connect, body, timeout).
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// Malformed URL during request construction.
    #[error("url parse: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Path is not prefixed by the expected root during directory walking.
    #[error("strip-prefix: {0}")]
    StripPrefix(#[from] std::path::StripPrefixError),

    /// Spawned tokio task panicked or was cancelled.
    #[error("join: {0}")]
    Join(#[from] tokio::task::JoinError),

    /// Free-form failure that does not fit a structured variant (e.g. a
    /// non-success HTTP status from the cache server).
    #[error("{0}")]
    Other(String),
}

/// Convenience alias used pervasively by `marmotte-cli` subcommands.
pub type CliResult<T> = Result<T, CliError>;

