//! `marmotte serve`: boot the HTTP server.
//

use std::path::PathBuf;

use marmotte_core::{auth::AuthSvc, config::Config, db::Db, gc::GcSvc, storage::LocalFsStore};
use marmotte_server::{build_app, observability::init_tracing, state::AppState};

use crate::error::CliResult;

/// Loads config, wires state, and binds the listener.
///
/// # Errors
///
/// Returns an error if the config file cannot be read, a component fails to
/// initialize, or the `TCP` listener cannot bind to the configured address.
pub async fn run(config: PathBuf) -> CliResult<()> {
    let cfg = Config::load(&config)?;
    init_tracing(&cfg.logging);
    tracing::info!(
        name: "server.start",
        config = ?config,
        listen = %cfg.server.listen,
        "starting marmotte: {{listen}}",
    );

    let db = Db::connect(&cfg.database).await?;
    let store = LocalFsStore::open(&cfg.server.storage_root).await?;
    let auth = AuthSvc::new(db.clone(), &cfg.auth);
    let gc = GcSvc::new(db.clone(), store.clone(), cfg.gc.clone());

    let state = AppState::new(cfg.clone(), db, store, auth, gc);
    let app = build_app(state.clone());

    let listener = tokio::net::TcpListener::bind(cfg.server.listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolves when `SIGINT` (Ctrl-C) or `SIGTERM` is received.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = term => {} }
    tracing::info!(name: "server.shutdown", "shutting down");
}
