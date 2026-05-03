//! HTTP route groups.
//!
//! Each submodule owns one logical route group that is merged into the
//! top-level [`axum::Router`] via [`crate::build_app`].
//

pub mod admin;
pub mod public;
pub mod yocto;
