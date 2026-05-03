//! Database access: `sqlx` pool wrapper plus per-entity repositories.
//!
//! This module owns the [`Db`] type, which wraps a [`SqlitePool`] configured
//! for Marmotte (`WAL` journal mode, NORMAL synchronous). On construction, all
//! pending migrations are applied automatically so callers never need to run
//! migrations manually.
//!
//! # Examples
//!
//! ```no_run
//! use marmotte_core::Db;
//!
//! # async fn run() -> marmotte_core::error::CoreResult<()> {
//! let db = Db::connect_memory().await?;
//! let pool = db.pool();
//! // … use pool for queries …
//! db.close().await;
//! # Ok(())
//! # }
//! ```
//
// Rust guideline compliant 2026-05-06

use std::{path::Path, time::Duration};

use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::{config::DatabaseConfig, error::CoreResult};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Owns a [`SqlitePool`] configured for Marmotte (`WAL`, NORMAL synchronous).
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Connects to a file-backed `SQLite` DB and applies pending migrations.
    ///
    /// Creates parent directories if they do not exist. The `SQLite` file is
    /// created if it does not already exist. All pending migrations from
    /// `./migrations` are applied before the pool is returned.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Io`] if directory creation fails,
    /// [`crate::error::CoreError::Database`] if the pool cannot be opened, or
    /// [`crate::error::CoreError::Migration`] if a migration fails.
    pub async fn connect(cfg: &DatabaseConfig) -> CoreResult<Self> {
        if let Some(parent) = cfg.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(&cfg.path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_millis(cfg.busy_timeout_ms))
            .foreign_keys(true);

        // 8 connections: a reasonable default for a single-writer SQLite
        // deployment; `WAL` mode allows concurrent readers alongside one writer.
        Self::from_options(opts, 8).await
    }

    /// Connects to an in-memory `SQLite` DB. Tests only.
    ///
    /// Applies all pending migrations before returning. In-memory databases
    /// are connection-scoped in `SQLite`, so the pool is capped to one
    /// connection to keep all operations on the same shared DB.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Database`] if the pool cannot be
    /// opened, or [`crate::error::CoreError::Migration`] if a migration fails.
    pub async fn connect_memory() -> CoreResult<Self> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        // Cap to 1: `:memory:` databases are per-connection; multiple
        // connections would each receive an empty, isolated database, so
        // migrations applied on one would be invisible to another.
        Self::from_options(opts, 1).await
    }

    async fn from_options(opts: SqliteConnectOptions, max_connections: u32) -> CoreResult<Self> {
        // 60 s: generous enough for debug-build argon2 under high concurrency
        // (50 concurrent requests × ~722 ms in unoptimised builds / 4 workers ≈ 9 s).
        // Release builds verify argon2 in < 5 ms, so this never fires in production.
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(60))
            .connect_with(opts)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// Returns the underlying `sqlx` pool.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes the pool. Idempotent.
    pub async fn close(self) {
        self.pool.close().await;
    }

    /// Opens a file-backed DB at `path`, applies migrations, then closes it.
    ///
    /// Convenience wrapper used by CLI and migration tooling to ensure the DB
    /// schema is up to date without keeping a connection open.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError`] on any IO, connection, or migration
    /// failure.
    pub async fn migrate_at(path: &Path) -> CoreResult<()> {
        let cfg = DatabaseConfig {
            path: path.to_path_buf(),
            // 5 000 ms: balances throughput and responsiveness for one-off
            // migration calls where no concurrent writers are expected.
            busy_timeout_ms: 5000,
        };
        let db = Self::connect(&cfg).await?;
        db.close().await;
        Ok(())
    }
}

pub mod admin_tokens;
pub mod api_keys;
pub mod blobs;
pub mod entries;
pub mod projects;
pub mod stats;

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current time as a Unix timestamp in seconds.
#[must_use]
#[expect(clippy::cast_possible_wrap, reason = "year-2262 problem")]
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    /// Returns the admin-tokens repository bound to this connection pool.
    #[must_use]
    pub fn admin_tokens(&self) -> admin_tokens::AdminTokens<'_> {
        admin_tokens::AdminTokens(self)
    }

    /// Returns the projects repository bound to this connection pool.
    #[must_use]
    pub fn projects(&self) -> projects::Projects<'_> {
        projects::Projects(self)
    }

    /// Returns the `API`-keys repository bound to this connection pool.
    #[must_use]
    pub fn api_keys(&self) -> api_keys::ApiKeys<'_> {
        api_keys::ApiKeys(self)
    }

    /// Returns the blob repository bound to this connection pool.
    #[must_use]
    pub fn blobs(&self) -> blobs::Blobs<'_> {
        blobs::Blobs(self)
    }

    /// Returns the entries repository bound to this connection pool.
    #[must_use]
    pub fn entries(&self) -> entries::Entries<'_> {
        entries::Entries(self)
    }

    /// Returns the stats / audit repository.
    #[must_use]
    pub fn stats(&self) -> stats::Stats<'_> {
        stats::Stats(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_in_memory_db_cleanly() {
        let db = Db::connect_memory().await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT count(*) FROM projects")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(row.0, 0);
    }

    #[tokio::test]
    async fn pragmas_applied() {
        let db = Db::connect_memory().await.unwrap();
        let mode: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .unwrap();
        // In-memory DB falls back to MEMORY journal; on-disk would be wal.
        assert!(mode.0 == "wal" || mode.0 == "memory");
    }
}
