//! Typed configuration with figment-based `TOML`+env loading.
//!
//! This module owns the single `Config` struct that drives every subsystem.
//! Configuration is loaded from a `TOML` file and then overlaid with
//! `MARMOTTE_*` environment variables using `__` as the hierarchy separator
//! (e.g. `MARMOTTE_SERVER__LISTEN=0.0.0.0:8080`).
//!
//! # Examples
//!
//! ```no_run
//! use marmotte_core::Config;
//!
//! let cfg = Config::load(std::path::Path::new("/etc/marmotte/config.toml"))
//!     .expect("failed to load configuration");
//! println!("listening on {}", cfg.server.listen);
//! ```
//

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// Top-level Marmotte configuration.
///
/// Loaded from a `TOML` file and overlaid with `MARMOTTE_*` environment
/// variables. See [`Config::load`] for details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// HTTP server settings (bind address, storage root, timeouts).
    pub server: ServerConfig,
    /// `SQLite` database settings.
    pub database: DatabaseConfig,
    /// Garbage-collection and quota settings.
    pub gc: GcConfig,
    /// Authentication and verification-cache settings.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Logging output settings.
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// HTTP server settings.
///
/// Controls the bind address, storage root, request timeout, and upload size
/// limit for the Marmotte HTTP layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// `SocketAddr` the server binds to (e.g. `"127.0.0.1:9090"`).
    pub listen: SocketAddr,
    /// Root directory for the content-addressed blob store.
    pub storage_root: PathBuf,
    /// Maximum seconds to wait for a single HTTP request to complete.
    ///
    /// Defaults to 300 s (5 minutes), which covers large Yocto artifact
    /// uploads on slow connections without tying up a thread indefinitely.
    #[serde(default = "ServerConfig::default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Maximum allowed upload body size in bytes.
    ///
    /// Defaults to 5 GiB (`5_368_709_120`), large enough for a full Yocto
    /// `sstate` tarball while still providing an upper-bound guard.
    #[serde(default = "ServerConfig::default_upload_max_bytes")]
    pub upload_max_bytes: u64,
}

impl ServerConfig {
    /// Default request timeout: 300 s covers large uploads on slow links.
    fn default_request_timeout_secs() -> u64 {
        300
    }

    /// Default upload limit: 5 GiB is enough for a full `sstate` tarball.
    fn default_upload_max_bytes() -> u64 {
        5_368_709_120
    }

    /// Returns the configured request timeout as a [`Duration`].
    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }
}

/// `SQLite` database settings.
///
/// `marmotte-core` uses a single `SQLite` file for metadata. These settings
/// tune the busy-timeout so concurrent writers back off rather than error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Filesystem path to the `SQLite` database file.
    pub path: PathBuf,
    /// Milliseconds to wait when the database is locked before returning an error.
    ///
    /// Defaults to 5000 ms. Increase on systems with many concurrent writers.
    #[serde(default = "DatabaseConfig::default_busy_timeout_ms")]
    pub busy_timeout_ms: u64,
}

impl DatabaseConfig {
    /// Default busy timeout: 5 000 ms balances throughput with responsiveness.
    fn default_busy_timeout_ms() -> u64 {
        5000
    }
}

/// Garbage-collection and quota settings.
///
/// Controls how frequently the GC sweep runs, per-layer TTLs, global storage
/// quota, and the high-watermark that triggers an early sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcConfig {
    /// Seconds between scheduled GC sweeps.
    ///
    /// Defaults to 300 s. Set lower for aggressive reclamation or higher to
    /// reduce I/O on write-heavy deployments.
    #[serde(default = "GcConfig::default_interval_secs")]
    pub interval_secs: u64,
    /// Default TTL in days for `sstate` cache entries.
    pub default_ttl_sstate_days: u64,
    /// Default TTL in days for `downloads` cache entries.
    pub default_ttl_downloads_days: u64,
    /// Hard storage quota in bytes across all projects.
    pub global_quota_bytes: u64,
    /// Quota utilization percentage that triggers an immediate GC sweep.
    ///
    /// Defaults to 90. Must be in the range `[1, 100]`.
    #[serde(default = "GcConfig::default_trigger_threshold_pct")]
    pub trigger_threshold_pct: u8,
}

impl GcConfig {
    /// Default GC sweep interval: 300 s (every 5 minutes).
    fn default_interval_secs() -> u64 {
        300
    }

    /// Default trigger threshold: 90% quota utilization starts an early sweep.
    fn default_trigger_threshold_pct() -> u8 {
        90
    }
}

/// Authentication and verification-cache settings.
///
/// Marmotte caches successful token verifications in a bounded `LRU`+TTL
/// cache to avoid repeated argon2 hashing on the hot path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Maximum number of entries in the token verification cache.
    ///
    /// Defaults to 1 024. Larger values trade memory for fewer re-hashes.
    #[serde(default = "AuthConfig::default_cache_size")]
    pub verify_cache_size: u64,
    /// Seconds before a cached verification result expires.
    ///
    /// Defaults to 300 s. Shorter TTLs reduce the window for revocation lag.
    #[serde(default = "AuthConfig::default_cache_ttl_secs")]
    pub verify_cache_ttl_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            verify_cache_size: Self::default_cache_size(),
            verify_cache_ttl_secs: Self::default_cache_ttl_secs(),
        }
    }
}

impl AuthConfig {
    /// Default cache size: 1 024 entries fit comfortably in a few MiB.
    fn default_cache_size() -> u64 {
        1024
    }

    /// Default cache TTL: 300 s limits revocation lag to 5 minutes.
    fn default_cache_ttl_secs() -> u64 {
        300
    }
}

/// Logging output settings.
///
/// Controls the minimum log level and structured output format emitted by the
/// `tracing` subscriber configured by the binary layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Minimum tracing level (e.g. `"info"`, `"debug"`, `"warn"`).
    ///
    /// Defaults to `"info"`. Accepts the same syntax as `RUST_LOG`.
    #[serde(default = "LoggingConfig::default_level")]
    pub level: String,
    /// Log output format emitted by the tracing subscriber.
    ///
    /// Defaults to [`LogFormat::Json`] for structured log pipelines.
    #[serde(default = "LoggingConfig::default_format")]
    pub format: LogFormat,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: Self::default_level(),
            format: Self::default_format(),
        }
    }
}

impl LoggingConfig {
    /// Default log level: `"info"` captures operational events without noise.
    fn default_level() -> String {
        "info".into()
    }

    /// Default log format: `Json` suits structured log pipelines.
    fn default_format() -> LogFormat {
        LogFormat::Json
    }
}

/// Log output format emitted by the `tracing` subscriber.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Structured `JSON` lines (default; suited for log aggregation pipelines).
    Json,
    /// Human-readable pretty output (suited for local development).
    Pretty,
}

impl Config {
    /// Loads configuration from `path` (`TOML`), then overlays `MARMOTTE_*` env vars.
    ///
    /// Environment variables use `__` as the hierarchy separator so that
    /// `MARMOTTE_SERVER__LISTEN=0.0.0.0:8080` overrides `[server] listen`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Config`] if the file cannot be read, is not valid
    /// `TOML`, or a required field is missing after env overlay.
    pub fn load(path: &std::path::Path) -> CoreResult<Self> {
        Figment::new()
            .merge(Toml::file(path))
            .merge(Env::prefixed("MARMOTTE_").split("__"))
            .extract()
            .map_err(CoreError::from)
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)] // figment::Error is 208 bytes; closure shape imposed by Jail::expect_with.
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config_minimal.toml")
    }

    #[test]
    fn loads_minimal_toml_fixture() {
        figment::Jail::expect_with(|_jail| {
            let cfg = Config::load(&fixture_path()).expect("load");
            assert_eq!(cfg.server.listen.port(), 9090);
            assert_eq!(cfg.database.busy_timeout_ms, 5000); // default kicks in
            assert_eq!(cfg.gc.default_ttl_sstate_days, 30);
            Ok(())
        });
    }

    #[test]
    fn env_var_overrides_toml() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("MARMOTTE_SERVER__LISTEN", "0.0.0.0:7777");
            let cfg = Config::load(&fixture_path()).expect("load");
            assert_eq!(cfg.server.listen.port(), 7777);
            Ok(())
        });
    }

    #[test]
    fn default_log_format_is_json() {
        figment::Jail::expect_with(|_jail| {
            let cfg = Config::load(&fixture_path()).expect("load");
            assert_eq!(cfg.logging.format, LogFormat::Json);
            Ok(())
        });
    }

    #[test]
    fn server_request_timeout_returns_duration() {
        figment::Jail::expect_with(|_jail| {
            let cfg = Config::load(&fixture_path()).expect("load");
            assert_eq!(
                cfg.server.request_timeout(),
                std::time::Duration::from_mins(5)
            );
            Ok(())
        });
    }
}
