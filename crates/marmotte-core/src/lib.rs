//! Core data model, storage, authentication, and GC engine for Marmotte.
//!
//! This crate intentionally has no HTTP or CLI dependencies. Higher layers
//! (`marmotte-server`, `marmotte-cli`) compose these primitives.
//!
//! # Modules
//! - [`error`] — the canonical [`error::CoreError`] enum.
//! - [`config`] — typed configuration plus parsing.
//! - [`models`] — domain types persisted to `SQLite`.
//! - [`db`] — sqlx pool wrapper and per-entity repositories.
//! - [`auth`] — argon2id hashing plus an LRU+TTL verification cache.
//! - [`storage`] — content-addressed local filesystem store.
//! - [`gc`] — TTL/quota/LRU sweep engine.
//

#![doc(html_no_source)]

pub mod error;

pub use error::{CoreError, CoreResult};

pub mod config;

#[doc(inline)]
pub use config::Config;

pub mod models;

#[doc(inline)]
pub use models::{AdminToken, ApiKey, Blob, Entry, Kind, Project, Role};

pub mod db;

#[doc(inline)]
pub use db::Db;

pub mod auth;

pub mod storage;

#[doc(inline)]
pub use storage::LocalFsStore;

pub mod gc;

#[doc(inline)]
pub use gc::{GcReport, GcSvc, OrphanReport};
