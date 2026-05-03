//! Shared test fixture.
#![allow(dead_code)]
//

use marmotte_core::{
    auth::{AuthSvc, generate_secret, hash_argon2, lookup_hash},
    config::{
        AuthConfig, Config, DatabaseConfig, GcConfig, LogFormat, LoggingConfig, ServerConfig,
    },
    db::Db,
    gc::GcSvc,
    models::Role,
    storage::LocalFsStore,
};
use marmotte_server::state::AppState;

/// The fully wired-up state and credentials created by [`fixture`] or
/// [`fixture_with_config`].
pub struct Fixture {
    pub state: AppState,
    pub project_name: String,
    pub api_key: String,
    pub admin_token: String,
}

/// Builds a minimal [`Fixture`] for testing.
///
/// Uses an in-memory `SQLite` database and a temporary directory for blob
/// storage. The `TempDir` is intentionally leaked so it outlives any async
/// executor teardown. `verify_cache_ttl_secs` is set to `0` so auth changes
/// are visible immediately in tests.
pub async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::connect_memory().await.unwrap();
    let store = LocalFsStore::open(dir.path()).await.unwrap();
    let mut cfg = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            storage_root: dir.path().to_path_buf(),
            request_timeout_secs: 5,
            upload_max_bytes: 1024 * 1024,
        },
        database: DatabaseConfig {
            path: ":memory:".into(),
            busy_timeout_ms: 0,
        },
        gc: GcConfig {
            interval_secs: 60,
            default_ttl_sstate_days: 1,
            default_ttl_downloads_days: 1,
            global_quota_bytes: 1024 * 1024,
            trigger_threshold_pct: 90,
        },
        auth: AuthConfig::default(),
        logging: LoggingConfig {
            level: "warn".into(),
            format: LogFormat::Json,
        },
    };
    // Zero TTL ensures revoke/grant changes are observed immediately in tests
    // without waiting for a cache entry to expire.
    cfg.auth.verify_cache_ttl_secs = 0;
    let auth = AuthSvc::new(db.clone(), &cfg.auth);
    let gc = GcSvc::new(db.clone(), store.clone(), cfg.gc.clone());

    let project = db.projects().create("acme", None, None).await.unwrap();
    let api_key = generate_secret();
    db.api_keys()
        .create(
            project.id,
            &lookup_hash(&api_key),
            &hash_argon2(&api_key).unwrap(),
            Role::Write,
            Some("ci"),
        )
        .await
        .unwrap();
    let admin_token = generate_secret();
    db.admin_tokens()
        .create(
            &lookup_hash(&admin_token),
            &hash_argon2(&admin_token).unwrap(),
            Some("root"),
        )
        .await
        .unwrap();

    // Box-leak keeps the temp dir alive until process exit so the underlying
    // path remains valid throughout the test.
    let _: &'static tempfile::TempDir = Box::leak(Box::new(dir));
    Fixture {
        state: AppState::new(cfg, db, store, auth, gc),
        project_name: project.name,
        api_key,
        admin_token,
    }
}

/// Builds a [`Fixture`] using a caller-supplied [`Config`].
///
/// The `storage_root` in `cfg.server` is overridden to match the temporary
/// directory so callers need not set it themselves.
pub async fn fixture_with_config(cfg: Config) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = cfg;
    cfg.server.storage_root = dir.path().to_path_buf();
    let db = Db::connect_memory().await.unwrap();
    let store = LocalFsStore::open(dir.path()).await.unwrap();
    let auth = AuthSvc::new(db.clone(), &cfg.auth);
    let gc = GcSvc::new(db.clone(), store.clone(), cfg.gc.clone());
    let project = db.projects().create("acme", None, None).await.unwrap();
    let api_key = generate_secret();
    db.api_keys()
        .create(
            project.id,
            &lookup_hash(&api_key),
            &hash_argon2(&api_key).unwrap(),
            Role::Write,
            Some("ci"),
        )
        .await
        .unwrap();
    let admin_token = generate_secret();
    db.admin_tokens()
        .create(
            &lookup_hash(&admin_token),
            &hash_argon2(&admin_token).unwrap(),
            Some("root"),
        )
        .await
        .unwrap();
    let _: &'static tempfile::TempDir = Box::leak(Box::new(dir));
    Fixture {
        state: AppState::new(cfg, db, store, auth, gc),
        project_name: project.name,
        api_key,
        admin_token,
    }
}
