//! Request and response payloads for the admin API.
//
// Rust guideline compliant 2026-05-06

use marmotte_core::models::{ApiKey, Project, Role};
use serde::{Deserialize, Serialize};

/// Request body for creating a new project.
#[derive(Debug, Deserialize)]
pub struct CreateProject {
    /// Unique name for the project.
    pub name: String,
    /// Optional storage quota in bytes.
    #[serde(default)]
    pub quota_bytes: Option<i64>,
    /// Optional time-to-live for entries in seconds.
    #[serde(default)]
    pub ttl_seconds: Option<i64>,
}

/// Request body for updating an existing project.
///
/// `Some(None)` clears the field; `Some(Some(n))` sets it; `None` leaves it unchanged.
// The double-option pattern is load-bearing: it distinguishes "field absent"
// (None) from "field explicitly set to null" (Some(None)).
#[allow(clippy::option_option)]
#[derive(Debug, Default, Deserialize)]
pub struct UpdateProject {
    /// New name for the project, if changing.
    pub name: Option<String>,
    /// `Some(None)` clears the quota; `Some(Some(n))` sets it; `None` leaves it.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub quota_bytes: Option<Option<i64>>,
    /// `Some(None)` clears the TTL; `Some(Some(n))` sets it; `None` leaves it.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub ttl_seconds: Option<Option<i64>>,
}

// The double-option is intentional: `Some(None)` clears the field, `Some(Some(v))`
// sets it, and `None` (field absent from JSON) leaves it unchanged.
#[allow(clippy::option_option)]
fn deserialize_double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(d).map(Some)
}

/// JSON representation of a project returned by the admin API.
#[derive(Debug, Serialize)]
pub struct ProjectView {
    /// Surrogate primary key.
    pub id: i64,
    /// Unique project name.
    pub name: String,
    /// Storage quota in bytes, if set.
    pub quota_bytes: Option<i64>,
    /// Entry TTL in seconds, if set.
    pub ttl_seconds: Option<i64>,
    /// Unix timestamp when the project was created.
    pub created_at: i64,
}

impl From<Project> for ProjectView {
    fn from(p: Project) -> Self {
        Self {
            id: p.id,
            name: p.name,
            quota_bytes: p.quota_bytes,
            ttl_seconds: p.ttl_seconds,
            created_at: p.created_at,
        }
    }
}

/// Request body for creating a new API key.
#[derive(Debug, Deserialize)]
pub struct CreateApiKey {
    /// Permission level for the new key.
    pub role: Role,
    /// Optional human-readable label.
    #[serde(default)]
    pub label: Option<String>,
}

/// Returned exactly once on key creation: the cleartext is in `secret`.
#[derive(Debug, Serialize)]
pub struct ApiKeyCreated {
    /// Surrogate primary key.
    pub id: i64,
    /// Owning project ID.
    pub project_id: i64,
    /// Permission level.
    pub role: Role,
    /// Human-readable label, if set.
    pub label: Option<String>,
    /// Unix timestamp when the key was created.
    pub created_at: i64,
    /// The only time this is ever returned in cleartext.
    pub secret: String,
}

/// JSON representation of an API key (without secret).
#[derive(Debug, Serialize)]
pub struct ApiKeyView {
    /// Surrogate primary key.
    pub id: i64,
    /// Owning project ID.
    pub project_id: i64,
    /// Permission level.
    pub role: Role,
    /// Human-readable label, if set.
    pub label: Option<String>,
    /// Unix timestamp when the key was created.
    pub created_at: i64,
    /// Unix timestamp when the key was revoked, if revoked.
    pub revoked_at: Option<i64>,
}

impl From<ApiKey> for ApiKeyView {
    fn from(k: ApiKey) -> Self {
        Self {
            id: k.id,
            project_id: k.project_id,
            role: k.role,
            label: k.label,
            created_at: k.created_at,
            revoked_at: k.revoked_at,
        }
    }
}

/// Request body for creating a new admin token.
#[derive(Debug, Deserialize)]
pub struct CreateAdminToken {
    /// Optional human-readable label.
    #[serde(default)]
    pub label: Option<String>,
}

/// Returned exactly once on admin token creation: the cleartext is in `secret`.
#[derive(Debug, Serialize)]
pub struct AdminTokenCreated {
    /// Surrogate primary key.
    pub id: i64,
    /// Human-readable label, if set.
    pub label: Option<String>,
    /// Unix timestamp when the token was created.
    pub created_at: i64,
    /// The only time the token secret is returned in cleartext.
    pub secret: String,
}

/// JSON representation of an admin token (without secret).
#[derive(Debug, Serialize)]
pub struct AdminTokenView {
    /// Surrogate primary key.
    pub id: i64,
    /// Human-readable label, if set.
    pub label: Option<String>,
    /// Unix timestamp when the token was created.
    pub created_at: i64,
    /// Unix timestamp when the token was revoked, if revoked.
    pub revoked_at: Option<i64>,
}
