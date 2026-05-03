//! Domain types persisted to `SQLite`. Strong typing for [`Role`] and [`Kind`].
//!
//! # Overview
//!
//! This module defines the core entity structs and enums used throughout
//! `marmotte-core`. All types are plain data; behavior is limited to the
//! `Role::allows_*` / `Kind::as_str` / `*::from_str` helpers.
//!
//! Structs are serializable via [`serde`] so they can be returned directly
//! from `sqlx` queries (via `sqlx::FromRow` in the `db` module) or serialized
//! to `JSON` in the admin `HTTP` `API`.
//!
//! # Types
//!
//! | Type | Purpose |
//! |------|---------|
//! | [`Role`] | Permission level attached to an [`ApiKey`]. |
//! | [`Kind`] | Cache namespace — `sstate` or `downloads`. |
//! | [`Project`] | Tenant record. |
//! | [`ApiKey`] | Project-scoped `HTTP` `Basic` credential. |
//! | [`AdminToken`] | Bearer token for the admin `API`. |
//! | [`Blob`] | Content-addressed binary object. |
//! | [`Entry`] | Logical `(project, kind, path) → blob` mapping. |
//
// Rust guideline compliant 2026-05-06

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// Permission level attached to an [`ApiKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Read-only access to blobs and entries.
    Read,
    /// Full read-write access.
    Write,
}

impl Role {
    /// Returns the textual representation stored in `SQLite`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use marmotte_core::Role;
    /// assert_eq!(Role::Read.as_str(), "read");
    /// assert_eq!(Role::Write.as_str(), "write");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    /// Parses a [`Role`] from its textual form.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidInput`] if `s` is not `"read"` or `"write"`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use marmotte_core::Role;
    /// assert_eq!(Role::from_str("write").unwrap(), Role::Write);
    /// assert!(Role::from_str("admin").is_err());
    /// ```
    // The plan specifies an inherent method rather than the `std::str::FromStr`
    // trait impl so callers can use the `CoreResult` return type directly.
    #[expect(
        clippy::should_implement_trait,
        reason = "Intentional inherent method; returns CoreResult rather than the trait's associated Error"
    )]
    pub fn from_str(s: &str) -> CoreResult<Self> {
        match s {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            other => Err(CoreError::InvalidInput(format!("unknown role: {other}"))),
        }
    }

    /// Returns `true` if this role permits read operations.
    ///
    /// Both [`Role::Read`] and [`Role::Write`] allow reads.
    #[must_use]
    pub fn allows_read(self) -> bool {
        matches!(self, Self::Read | Self::Write)
    }

    /// Returns `true` if this role permits write operations.
    ///
    /// Only [`Role::Write`] allows writes.
    #[must_use]
    pub fn allows_write(self) -> bool {
        matches!(self, Self::Write)
    }
}

/// Cache namespace; determines the `URL` path segment and storage layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Yocto shared-state cache (`sstate-cache`).
    Sstate,
    /// Yocto downloads mirror.
    Downloads,
}

impl Kind {
    /// Returns the textual representation used in `URL` paths and `SQLite`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use marmotte_core::Kind;
    /// assert_eq!(Kind::Sstate.as_str(), "sstate");
    /// assert_eq!(Kind::Downloads.as_str(), "downloads");
    /// ```
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sstate => "sstate",
            Self::Downloads => "downloads",
        }
    }

    /// Parses a [`Kind`] from its `URL`/string form.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidInput`] if `s` is not a known kind.
    ///
    /// # Examples
    ///
    /// ```
    /// # use marmotte_core::Kind;
    /// assert_eq!(Kind::from_str("downloads").unwrap(), Kind::Downloads);
    /// assert!(Kind::from_str("npm").is_err());
    /// ```
    // The plan specifies an inherent method rather than the `std::str::FromStr`
    // trait impl so callers can use the `CoreResult` return type directly.
    #[expect(
        clippy::should_implement_trait,
        reason = "Intentional inherent method; returns CoreResult rather than the trait's associated Error"
    )]
    pub fn from_str(s: &str) -> CoreResult<Self> {
        match s {
            "sstate" => Ok(Self::Sstate),
            "downloads" => Ok(Self::Downloads),
            other => Err(CoreError::InvalidInput(format!("unknown kind: {other}"))),
        }
    }
}

/// A tenant (build project).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Surrogate primary key.
    pub id: i64,
    /// Human-readable project name; unique across all projects.
    pub name: String,
    /// Maximum total blob storage in bytes; `None` means unlimited.
    pub quota_bytes: Option<i64>,
    /// Maximum age of entries in seconds; `None` means entries never expire.
    pub ttl_seconds: Option<i64>,
    /// Unix timestamp (seconds) when this project was created.
    pub created_at: i64,
}

/// A project-scoped `API` key used for `HTTP` `Basic` authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    /// Surrogate primary key.
    pub id: i64,
    /// Foreign key to the owning [`Project`].
    pub project_id: i64,
    /// `SHA-256` hex of the raw secret; used as a fast lookup index.
    pub key_lookup: String,
    /// Argon2id verifier of the raw secret; used for the slow verification path.
    pub key_hash: String,
    /// Permission level granted by this key.
    pub role: Role,
    /// Optional human-readable label for display in the admin `API`.
    pub label: Option<String>,
    /// Unix timestamp (seconds) when this key was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) when this key was revoked; `None` means active.
    pub revoked_at: Option<i64>,
}

/// Bearer token for the admin `API`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminToken {
    /// Surrogate primary key.
    pub id: i64,
    /// `SHA-256` hex of the raw token; used as a fast lookup index.
    pub token_lookup: String,
    /// Argon2id verifier of the raw token; used for the slow verification path.
    pub token_hash: String,
    /// Optional human-readable label for display purposes.
    pub label: Option<String>,
    /// Unix timestamp (seconds) when this token was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) when this token was revoked; `None` means active.
    pub revoked_at: Option<i64>,
}

/// Content-addressed binary object stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blob {
    /// `SHA-256` hex digest that uniquely identifies this blob's content.
    pub hash: String,
    /// Size of the blob in bytes.
    pub size_bytes: i64,
    /// Number of [`Entry`] rows that reference this blob.
    pub refcount: i64,
    /// Unix timestamp (seconds) when this blob was first uploaded.
    pub created_at: i64,
}

/// Logical entry mapping `(project, kind, path)` to a [`Blob`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Surrogate primary key.
    pub id: i64,
    /// Foreign key to the owning [`Project`].
    pub project_id: i64,
    /// Cache namespace this entry belongs to.
    pub kind: Kind,
    /// Relative path within the cache namespace (e.g. `universal/foo-1.0.tgz`).
    pub path: String,
    /// `SHA-256` hex of the referenced [`Blob`].
    pub blob_hash: String,
    /// Cached size of the referenced blob in bytes.
    pub size_bytes: i64,
    /// Unix timestamp (seconds) when this entry was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) of the most recent `GET` for this entry.
    pub last_accessed: i64,
    /// When `true`, the `GC` engine will never evict this entry.
    pub pinned: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_round_trips_via_str() {
        assert_eq!(Role::Read.as_str(), "read");
        assert_eq!(Role::Write.as_str(), "write");
        assert_eq!(Role::from_str("read").unwrap(), Role::Read);
        assert_eq!(Role::from_str("write").unwrap(), Role::Write);
        assert!(Role::from_str("admin").is_err());
    }

    #[test]
    fn kind_round_trips_via_str() {
        assert_eq!(Kind::Sstate.as_str(), "sstate");
        assert_eq!(Kind::Downloads.as_str(), "downloads");
        assert_eq!(Kind::from_str("sstate").unwrap(), Kind::Sstate);
        assert!(Kind::from_str("npm").is_err());
    }

    #[test]
    fn write_role_implies_read() {
        assert!(Role::Write.allows_read());
        assert!(Role::Read.allows_read());
        assert!(Role::Write.allows_write());
        assert!(!Role::Read.allows_write());
    }
}
