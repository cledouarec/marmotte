//! Reusable test harness: spawns a real Marmotte server on a random port.
//

// Test-only crate: public items are harness helpers, not a public API.
#![expect(missing_docs, reason = "test harness; documentation not required")]

use std::sync::Arc;

use marmotte_core::{
    auth::{AuthSvc, generate_secret, hash_argon2, lookup_hash},
    config::{AuthConfig, Config, DatabaseConfig, GcConfig, LoggingConfig, ServerConfig},
    db::Db,
    gc::GcSvc,
    models::Role,
    storage::LocalFsStore,
};
use marmotte_server::{build_app, state::AppState};
use reqwest::Url;
use tempfile::TempDir;
use thiserror::Error;

/// Errors raised while bringing up the test [`Harness`].
#[derive(Debug, Error)]
pub enum HarnessError {
    /// Failure originating from `marmotte-core` (config, db, storage, auth).
    #[error(transparent)]
    Core(#[from] marmotte_core::error::CoreError),

    /// Filesystem or socket I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Listener address could not be parsed.
    #[error("addr parse: {0}")]
    AddrParse(#[from] std::net::AddrParseError),

    /// Test base URL could not be parsed.
    #[error("url parse: {0}")]
    UrlParse(#[from] url::ParseError),
}

/// Convenience alias for harness setup operations.
pub type HarnessResult<T> = Result<T, HarnessError>;

pub struct Harness {
    pub url: Url,
    pub project: String,
    pub api_key: String,
    pub admin_token: String,
    pub state: AppState,
    _dir: Arc<TempDir>,
}

impl Harness {
    /// Spawns a fresh in-memory Marmotte server bound to a random local port,
    /// pre-provisioned with one project, one write-scoped API key, and one
    /// admin token. Returns a [`Harness`] handle exposing the server URL and
    /// the credentials needed by the test client.
    ///
    /// # Errors
    ///
    /// Returns a [`HarnessError`] if the temporary directory cannot be
    /// created, the in-memory database or storage backend cannot be opened,
    /// the loopback listener cannot be bound, the seeded project or
    /// credentials cannot be persisted, or the test base URL cannot be
    /// parsed.
    pub async fn spawn() -> HarnessResult<Self> {
        let dir = Arc::new(tempfile::tempdir()?);
        let db = Db::connect_memory().await?;
        let store = LocalFsStore::open(dir.path()).await?;
        let mut cfg = Config {
            server: ServerConfig {
                listen: "127.0.0.1:0".parse()?,
                storage_root: dir.path().to_path_buf(),
                request_timeout_secs: 30,
                upload_max_bytes: 16 * 1024 * 1024,
            },
            database: DatabaseConfig {
                path: ":memory:".into(),
                busy_timeout_ms: 0,
            },
            gc: GcConfig {
                interval_secs: 9999,
                default_ttl_sstate_days: 365,
                default_ttl_downloads_days: 365,
                global_quota_bytes: 16 * 1024 * 1024,
                trigger_threshold_pct: 90,
            },
            auth: AuthConfig::default(),
            logging: LoggingConfig::default(),
        };
        cfg.auth.verify_cache_ttl_secs = 0;

        let project = db.projects().create("acme", None, None).await?;
        let api_key = generate_secret();
        db.api_keys()
            .create(
                project.id,
                &lookup_hash(&api_key),
                &hash_argon2(&api_key)?,
                Role::Write,
                None,
            )
            .await?;
        let admin_token = generate_secret();
        db.admin_tokens()
            .create(
                &lookup_hash(&admin_token),
                &hash_argon2(&admin_token)?,
                None,
            )
            .await?;

        let auth = AuthSvc::new(db.clone(), &cfg.auth);
        let gc = GcSvc::new(db.clone(), store.clone(), cfg.gc.clone());
        let state = AppState::new(cfg.clone(), db, store, auth, gc);

        let listener = tokio::net::TcpListener::bind(cfg.server.listen).await?;
        let port = listener.local_addr()?.port();
        let app = build_app(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Ok(Self {
            url: format!("http://127.0.0.1:{port}").parse()?,
            project: project.name,
            api_key,
            admin_token,
            state,
            _dir: dir,
        })
    }

    /// Builds an HTTP `Authorization: Basic <base64(project:api_key)>` header
    /// value for the seeded project's write-scoped API key.
    ///
    /// # Panics
    ///
    /// Panics if the encoded credentials are not a valid `HeaderValue`. The
    /// project name and API key are produced by the harness from ASCII-safe
    /// inputs, so this branch is unreachable in practice.
    #[must_use]
    pub fn project_basic(&self) -> reqwest::header::HeaderValue {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let raw = format!(
            "Basic {}",
            STANDARD.encode(format!("{}:{}", self.project, self.api_key))
        );
        raw.try_into().unwrap()
    }

    /// Builds an HTTP `Authorization: Bearer <token>` header value for the
    /// seeded admin token.
    ///
    /// # Panics
    ///
    /// Panics if the formatted bearer string is not a valid `HeaderValue`.
    /// The admin token is generated by the harness from ASCII-safe bytes, so
    /// this branch is unreachable in practice.
    #[must_use]
    pub fn admin_bearer(&self) -> reqwest::header::HeaderValue {
        format!("Bearer {}", self.admin_token).try_into().unwrap()
    }
}
