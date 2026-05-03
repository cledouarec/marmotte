//! End-to-end push test against an in-process server.
//

use std::process::Command;

use marmotte_core::{
    auth::{generate_secret, hash_argon2, lookup_hash},
    config::{AuthConfig, Config, DatabaseConfig, GcConfig, LoggingConfig, ServerConfig},
    db::Db,
    gc::GcSvc,
    models::Role,
    storage::LocalFsStore,
};
use marmotte_server::build_app;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_uploads_files() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::connect_memory().await.unwrap();
    let store = LocalFsStore::open(dir.path()).await.unwrap();
    let cfg = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            storage_root: dir.path().to_path_buf(),
            request_timeout_secs: 10,
            upload_max_bytes: 1 << 20,
        },
        database: DatabaseConfig {
            path: ":memory:".into(),
            busy_timeout_ms: 0,
        },
        gc: GcConfig {
            interval_secs: 9999,
            default_ttl_sstate_days: 1,
            default_ttl_downloads_days: 1,
            global_quota_bytes: 1 << 20,
            trigger_threshold_pct: 90,
        },
        auth: AuthConfig::default(),
        logging: LoggingConfig::default(),
    };
    let auth = marmotte_core::auth::AuthSvc::new(db.clone(), &cfg.auth);
    let gc = GcSvc::new(db.clone(), store.clone(), cfg.gc.clone());
    let project = db.projects().create("acme", None, None).await.unwrap();
    let secret = generate_secret();
    db.api_keys()
        .create(
            project.id,
            &lookup_hash(&secret),
            &hash_argon2(&secret).unwrap(),
            Role::Write,
            None,
        )
        .await
        .unwrap();

    let state = marmotte_server::state::AppState::new(cfg, db, store, auth, gc);
    let app = build_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Build a source tree with two files in a nested directory.
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("a/b")).unwrap();
    std::fs::write(src.path().join("a/b/c.txt"), b"hello").unwrap();
    std::fs::write(src.path().join("a/d.txt"), b"world").unwrap();

    let exe = env!("CARGO_BIN_EXE_marmotte");
    let status = Command::new(exe)
        .args([
            "push",
            "--project",
            "acme",
            "--kind",
            "sstate",
            "--base-url",
            &format!("http://127.0.0.1:{port}/"),
            "--api-key",
            &secret,
            src.path().to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
}

