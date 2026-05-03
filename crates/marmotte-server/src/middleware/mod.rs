//! Tower middleware for request authentication.
//
// Rust guideline compliant 2026-05-06

/// Admin Bearer-token middleware.
pub mod auth_admin;
/// Project HTTP Basic-auth middleware.
pub mod auth_project;
