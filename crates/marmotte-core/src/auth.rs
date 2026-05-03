//! Authentication primitives: secret generation, argon2id hashing, and a
//! moka-backed verification cache.
//!
//! Marmotte uses a two-column scheme for both `API` keys and admin tokens:
//! a fast, deterministic `SHA-256` *lookup* hash (indexable, used to find the
//! row) and a slow argon2id *verifier* hash (constant-time validation). The
//! moka cache amortizes argon2 across many hits of the same secret.
//!
//! # Examples
//!
//! ```no_run
//! use marmotte_core::auth::{generate_secret, hash_argon2, lookup_hash, verify_argon2};
//!
//! let secret = generate_secret();
//! let lookup = lookup_hash(&secret);
//! let phc = hash_argon2(&secret).expect("hash");
//! assert!(verify_argon2(&secret, &phc).expect("verify"));
//! ```
//

use std::time::Duration;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, Salt, SaltString},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use moka::future::Cache;
use sha2::{Digest, Sha256};

use crate::{
    config::AuthConfig,
    error::{CoreError, CoreResult},
};

/// Generates a 256-bit random secret encoded as `URL`-safe base64 (≈43 chars).
///
/// Uses [`rand::fill`] (the thread-local CSPRNG seeded from the OS) for
/// cryptographically secure randomness. The result is safe for use as an `API`
/// key or admin token; store only its [`lookup_hash`] and [`hash_argon2`]
/// output, never the raw secret.
#[must_use]
pub fn generate_secret() -> String {
    // 32 bytes = 256 bits of entropy; URL_SAFE_NO_PAD encodes to 43 chars.
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Computes the `SHA-256` lookup hash of a secret as a lowercase hex string.
///
/// The lookup hash is deterministic and fast. It is stored in an indexed
/// database column so rows can be located without a full table scan. Because
/// `SHA-256` is not a password hash, the argon2id verifier ([`hash_argon2`])
/// must always be stored alongside it.
#[must_use]
pub fn lookup_hash(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    hex::encode(h.finalize())
}

/// Produces an argon2id `PHC`-format hash of `secret` with a fresh random salt.
///
/// The returned string is suitable for persistent storage. Use [`verify_argon2`]
/// to validate a candidate secret against it.
///
/// # Errors
///
/// Returns [`crate::error::CoreError::Crypto`] if the underlying argon2
/// hashing operation fails (e.g., an invalid salt was generated).
pub fn hash_argon2(secret: &str) -> CoreResult<String> {
    // password_hash 0.5 still uses rand_core 0.6, which is incompatible with
    // rand 0.10's trait set, so SaltString::generate() can no longer accept
    // our RNG. Generate the recommended-length salt ourselves and encode it.
    let mut salt_bytes = [0u8; Salt::RECOMMENDED_LENGTH];
    rand::fill(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)?;
    let phc = Argon2::default()
        .hash_password(secret.as_bytes(), &salt)?
        .to_string();
    Ok(phc)
}

/// Verifies a candidate `secret` against a stored argon2id `PHC` string.
///
/// Returns `true` when the secret matches, `false` when it does not.
/// The comparison is performed in constant time by the argon2 library.
///
/// # Errors
///
/// Returns [`crate::error::CoreError::Crypto`] if `phc` is not a valid
/// `PHC`-format string or if the underlying verification engine errors.
pub fn verify_argon2(secret: &str, phc: &str) -> CoreResult<bool> {
    let parsed = PasswordHash::new(phc)?;
    Ok(Argon2::default()
        .verify_password(secret.as_bytes(), &parsed)
        .is_ok())
}

/// Async wrapper for [`verify_argon2`] that offloads `CPU`-intensive hashing
/// to a `spawn_blocking` thread so tokio workers remain available.
///
/// Argon2 verification is intentionally slow (`>100 ms` in debug builds,
/// `>5 ms` in release). Running it synchronously on a tokio worker starves the
/// runtime under concurrent load; `spawn_blocking` moves the work to a
/// dedicated blocking thread pool.
///
/// # Errors
///
/// Returns [`crate::error::CoreError::Crypto`] if argon2 verification fails or
/// [`crate::error::CoreError::Internal`] if the blocking task panics.
pub async fn verify_argon2_async(secret: String, phc: String) -> CoreResult<bool> {
    tokio::task::spawn_blocking(move || verify_argon2(&secret, &phc))
        .await
        .map_err(|e| CoreError::Internal(format!("argon2 task panicked: {e}")))?
}

/// Constant-time string equality.
///
/// Wraps [`subtle::ConstantTimeEq`] so callers do not need to depend on
/// `subtle` directly. Use this on the authentication hot path to prevent
/// timing-based side-channel attacks.
#[must_use]
pub fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Moka-backed cache that amortizes argon2 verification across repeated calls.
///
/// The key is `(scope, lookup_hash)` where `scope` is a short string
/// distinguishing token types (e.g., `"api_key"`, `"admin_token"`). The value
/// `V` is whatever the caller stores per successful verification (typically a
/// role or permission set).
///
/// Build one instance per process via [`VerifyCache::from_config`] and share it
/// behind an `Arc`.
#[derive(Clone)]
pub struct VerifyCache<V: Clone + Send + Sync + 'static> {
    inner: Cache<(String, String), V>,
}

impl<V: Clone + Send + Sync + 'static> std::fmt::Debug for VerifyCache<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifyCache")
            .field("entry_count", &self.inner.entry_count())
            .finish()
    }
}

impl<V: Clone + Send + Sync + 'static> VerifyCache<V> {
    /// Builds a [`VerifyCache`] sized and aged according to [`AuthConfig`].
    #[must_use]
    pub fn from_config(cfg: &AuthConfig) -> Self {
        Self {
            inner: Cache::builder()
                .max_capacity(cfg.verify_cache_size)
                .time_to_live(Duration::from_secs(cfg.verify_cache_ttl_secs))
                .build(),
        }
    }

    /// Returns a cached value or runs `f`, caching and returning its `Ok` result.
    ///
    /// On a cache miss, `f` is called once. Its `Ok` value is inserted and
    /// returned. Errors from `f` are propagated without being cached.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by `f`.
    pub async fn get_or_insert<F, Fut>(&self, scope: &str, lookup: &str, f: F) -> CoreResult<V>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = CoreResult<V>>,
    {
        let key = (scope.to_owned(), lookup.to_owned());
        if let Some(v) = self.inner.get(&key).await {
            return Ok(v);
        }
        let v = f().await?;
        self.inner.insert(key, v.clone()).await;
        Ok(v)
    }
}

/// Outcome of a successful project authentication.
///
/// Returned by [`AuthSvc::authenticate_project`] on success. Carries the
/// minimal set of facts the `HTTP` layer needs to authorize the request.
#[derive(Debug, Clone)]
pub struct ProjectAuth {
    /// `SQLite` primary key of the authenticated project.
    pub project_id: i64,
    /// Human-readable name of the authenticated project.
    pub project_name: String,
    /// `SQLite` primary key of the `API` key that was matched.
    pub api_key_id: i64,
    /// Permission level granted by the matched `API` key.
    pub role: crate::models::Role,
}

/// Outcome of a successful admin authentication.
///
/// Returned by [`AuthSvc::authenticate_admin`] on success.
#[derive(Debug, Clone)]
pub struct AdminAuth {
    /// `SQLite` primary key of the matched admin token.
    pub token_id: i64,
    /// Optional human-readable label attached to the token.
    pub token_label: Option<String>,
}

/// Stateful auth service: holds the verify cache and a [`crate::db::Db`] handle.
///
/// Create one instance per process and share it via `Arc`/`Clone`. Both the
/// project cache and the admin cache are backed by moka and bounded by the
/// parameters in [`AuthConfig`].
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> marmotte_core::error::CoreResult<()> {
/// use marmotte_core::{auth::AuthSvc, config::AuthConfig, Db};
/// let db = Db::connect_memory().await?;
/// let svc = AuthSvc::new(db, &AuthConfig::default());
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct AuthSvc {
    db: crate::db::Db,
    project_cache: VerifyCache<ProjectAuth>,
    admin_cache: VerifyCache<AdminAuth>,
}

impl std::fmt::Debug for AuthSvc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthSvc")
            .field("project_cache", &self.project_cache)
            .field("admin_cache", &self.admin_cache)
            .finish_non_exhaustive()
    }
}

impl AuthSvc {
    /// Builds a new auth service backed by `db` and sized by `cfg`.
    #[must_use]
    pub fn new(db: crate::db::Db, cfg: &AuthConfig) -> Self {
        Self {
            db,
            project_cache: VerifyCache::from_config(cfg),
            admin_cache: VerifyCache::from_config(cfg),
        }
    }

    /// Authenticates a project `Basic`-auth pair `(project_name, api_key)`.
    ///
    /// The happy path is:
    /// 1. Look up the project by `project_name`.
    /// 2. Derive the `SHA-256` lookup hash of `api_key` and find the active key row.
    /// 3. Verify the raw secret against the argon2id verifier in the row.
    /// 4. Cache and return a [`ProjectAuth`] on success.
    ///
    /// Any step that fails (unknown project, no matching active key, wrong
    /// secret) returns [`crate::error::CoreError::Unauthorized`]. Database
    /// errors (other than "not found") propagate as
    /// [`crate::error::CoreError::Database`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Unauthorized`] when the credentials
    /// are absent, wrong, or revoked.
    /// Returns [`crate::error::CoreError::Crypto`] if argon2 verification fails
    /// at the engine level.
    /// Returns [`crate::error::CoreError::Database`] on a query failure.
    pub async fn authenticate_project(
        &self,
        project_name: &str,
        api_key: &str,
    ) -> CoreResult<ProjectAuth> {
        let lookup = lookup_hash(api_key);
        // Scope distinguishes keys per-project so one project's cached result
        // cannot satisfy another project's lookup.
        let scope = format!("proj:{project_name}");
        self.project_cache
            .get_or_insert(&scope, &lookup, || async {
                let project = self
                    .db
                    .projects()
                    .get_by_name(project_name)
                    .await
                    .map_err(|_| CoreError::Unauthorized)?;
                let row = self
                    .db
                    .api_keys()
                    .find_active_by_lookup(project.id, &lookup)
                    .await?;
                let key = row.ok_or(CoreError::Unauthorized)?;
                if !verify_argon2_async(api_key.to_owned(), key.key_hash.clone()).await? {
                    return Err(CoreError::Unauthorized);
                }
                Ok(ProjectAuth {
                    project_id: project.id,
                    project_name: project.name,
                    api_key_id: key.id,
                    role: key.role,
                })
            })
            .await
    }

    /// Authenticates a Bearer admin token.
    ///
    /// The happy path is:
    /// 1. Derive the `SHA-256` lookup hash of `token`.
    /// 2. Find the active admin-token row.
    /// 3. Verify the raw token against the argon2id verifier.
    /// 4. Cache and return an [`AdminAuth`] on success.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Unauthorized`] when the token is
    /// absent, wrong, or revoked.
    /// Returns [`crate::error::CoreError::Crypto`] if argon2 verification fails
    /// at the engine level.
    /// Returns [`crate::error::CoreError::Database`] on a query failure.
    pub async fn authenticate_admin(&self, token: &str) -> CoreResult<AdminAuth> {
        let lookup = lookup_hash(token);
        self.admin_cache
            .get_or_insert("admin", &lookup, || async {
                let row = self
                    .db
                    .admin_tokens()
                    .find_active_by_lookup(&lookup)
                    .await?;
                let t = row.ok_or(CoreError::Unauthorized)?;
                if !verify_argon2_async(token.to_owned(), t.token_hash.clone()).await? {
                    return Err(CoreError::Unauthorized);
                }
                Ok(AdminAuth {
                    token_id: t.id,
                    token_label: t.label,
                })
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_secret_is_url_safe_and_long() {
        let s = generate_secret();
        assert!(s.len() >= 40);
        assert!(
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn lookup_is_stable_and_hex() {
        let h1 = lookup_hash("abcdef");
        let h2 = lookup_hash("abcdef");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn argon2_round_trip() {
        let secret = "mar_test_secret";
        let h = hash_argon2(secret).unwrap();
        assert!(verify_argon2(secret, &h).unwrap());
        assert!(!verify_argon2("other", &h).unwrap());
    }

    use crate::{config::AuthConfig, db::Db, error::CoreError, models::Role};

    #[tokio::test]
    async fn authenticate_project_with_valid_key() {
        let db = Db::connect_memory().await.unwrap();
        let p = db.projects().create("acme", None, None).await.unwrap();
        let secret = generate_secret();
        let lookup = lookup_hash(&secret);
        let phc = hash_argon2(&secret).unwrap();
        db.api_keys()
            .create(p.id, &lookup, &phc, Role::Write, None)
            .await
            .unwrap();

        let svc = AuthSvc::new(db, &AuthConfig::default());
        let ok = svc.authenticate_project("acme", &secret).await.unwrap();
        assert_eq!(ok.role, Role::Write);
    }

    #[tokio::test]
    async fn authenticate_project_rejects_wrong_key() {
        let db = Db::connect_memory().await.unwrap();
        let p = db.projects().create("acme", None, None).await.unwrap();
        let secret = generate_secret();
        let lookup = lookup_hash(&secret);
        let phc = hash_argon2(&secret).unwrap();
        db.api_keys()
            .create(p.id, &lookup, &phc, Role::Read, None)
            .await
            .unwrap();

        let svc = AuthSvc::new(db, &AuthConfig::default());
        let e = svc.authenticate_project("acme", "wrong").await.unwrap_err();
        assert!(matches!(e, CoreError::Unauthorized), "got {e:?}");
    }

    #[tokio::test]
    async fn authenticate_admin_round_trip() {
        let db = Db::connect_memory().await.unwrap();
        let secret = generate_secret();
        let lookup = lookup_hash(&secret);
        let phc = hash_argon2(&secret).unwrap();
        db.admin_tokens()
            .create(&lookup, &phc, Some("ci"))
            .await
            .unwrap();

        let svc = AuthSvc::new(db, &AuthConfig::default());
        let a = svc.authenticate_admin(&secret).await.unwrap();
        assert_eq!(a.token_label.as_deref(), Some("ci"));
    }
}
