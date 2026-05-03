//! HTTP route groups.
//!
//! Each submodule owns one logical route group that is merged into the
//! top-level [`axum::Router`] via [`crate::build_app`].
//
// Rust guideline compliant 2026-05-06

pub mod admin;
pub mod public;
pub mod yocto;
