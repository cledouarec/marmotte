//! Canonical error type for `marmotte-core`.
//!
//! Higher layers map [`CoreError`] to their own protocol (e.g. HTTP status
//! codes via `axum::response::IntoResponse` in `marmotte-server`). Variants
//! carry just enough context to render a useful message; never include
//! secrets (raw API keys, argon2 hashes, etc.).
//
// Rust guideline compliant 2026-05-06

use thiserror::Error;

/// Errors emitted by `marmotte-core`.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Resource not found (e.g. unknown project, entry, blob).
    #[error("not found: {what}")]
    NotFound {
        /// Human-readable description of the missing resource.
        what: String,
    },

    /// Caller authenticated but not allowed to perform the action.
    #[error("forbidden: {reason}")]
    Forbidden {
        /// Explanation of why the operation was denied.
        reason: String,
    },

    /// Caller could not be authenticated.
    #[error("unauthorized")]
    Unauthorized,

    /// Conflict (e.g. duplicate project name, in-flight blob).
    #[error("conflict: {what}")]
    Conflict {
        /// Human-readable description of the conflicting resource.
        what: String,
    },

    /// Validation error on user-provided input.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Quota would be exceeded by this operation (soft warning, not blocking).
    #[error("quota: {0}")]
    Quota(String),

    /// Underlying database failure.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Database migration failure.
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    /// Filesystem / IO error from the storage backend.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Hashing or password-verification failure (argon2 / sha256).
    #[error("crypto error: {0}")]
    Crypto(String),

    /// Configuration load/parse failure.
    #[error("config error: {0}")]
    Config(String),

    /// Anything that doesn't fit elsewhere; prefer adding a variant.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias used pervasively in `marmotte-core`.
pub type CoreResult<T> = Result<T, CoreError>;

impl From<argon2::password_hash::Error> for CoreError {
    fn from(e: argon2::password_hash::Error) -> Self {
        CoreError::Crypto(e.to_string())
    }
}

impl From<figment::Error> for CoreError {
    fn from(e: figment::Error) -> Self {
        CoreError::Config(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::CoreError;

    #[test]
    fn not_found_renders_a_human_message() {
        let e = CoreError::NotFound {
            what: "entry: foo/bar".into(),
        };
        assert_eq!(e.to_string(), "not found: entry: foo/bar");
    }

    #[test]
    fn forbidden_carries_reason() {
        let e = CoreError::Forbidden {
            reason: "cross-project access".into(),
        };
        assert!(e.to_string().contains("cross-project access"));
    }

    #[test]
    fn from_sqlx_maps_to_database() {
        let inner = sqlx::Error::RowNotFound;
        let e: CoreError = inner.into();
        assert!(matches!(e, CoreError::Database(_)));
    }
}
