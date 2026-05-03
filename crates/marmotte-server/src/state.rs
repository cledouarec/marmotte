//! Shared application state passed to handlers.
//

use std::sync::Arc;

use marmotte_core::{auth::AuthSvc, config::Config, db::Db, gc::GcSvc, storage::LocalFsStore};

/// Shared, cheaply cloneable state.
#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

/// Owned state held by the [`AppState`] arc.
pub struct AppStateInner {
    /// Loaded Marmotte configuration.
    pub config: Config,
    /// `SQLite` database handle.
    pub db: Db,
    /// Content-addressed local filesystem store.
    pub store: LocalFsStore,
    /// Authentication service with argon2id + LRU cache.
    pub auth: AuthSvc,
    /// Garbage-collection and quota sweep service.
    pub gc: GcSvc,
}

impl AppState {
    /// Constructs a new application state from already-wired components.
    #[must_use]
    pub fn new(config: Config, db: Db, store: LocalFsStore, auth: AuthSvc, gc: GcSvc) -> Self {
        Self(Arc::new(AppStateInner {
            config,
            db,
            store,
            auth,
            gc,
        }))
    }
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
