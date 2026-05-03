//! `api_keys` table repository. Lookup is by `SHA-256` (`key_lookup`); the
//! argon2 verifier (`key_hash`) is fetched alongside for verification.
//
// Rust guideline compliant 2026-05-06

use sqlx::Row;

use crate::{
    db::Db,
    error::{CoreError, CoreResult},
    models::{ApiKey, Role},
};

/// `API`-key repository — CRUD operations on the `api_keys` table.
///
/// Obtain an instance via [`Db::api_keys`].
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> marmotte_core::error::CoreResult<()> {
/// use marmotte_core::{Db, models::Role};
/// let db = Db::connect_memory().await?;
/// let key = db.api_keys().create(1, "sha256hex", "argon2hash", Role::Read, Some("ci")).await?;
/// # Ok(())
/// # }
/// ```
pub struct ApiKeys<'a>(pub(crate) &'a Db);

impl ApiKeys<'_> {
    /// Inserts a new `API` key and returns the persisted [`ApiKey`].
    ///
    /// `key_lookup` must be unique per project — a `UNIQUE` constraint on
    /// `(project_id, key_lookup)` is enforced at the database level.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on any `SQLite` failure, including
    /// a `UNIQUE` constraint violation when `key_lookup` is already in use.
    pub async fn create(
        &self,
        project_id: i64,
        key_lookup: &str,
        key_hash: &str,
        role: Role,
        label: Option<&str>,
    ) -> CoreResult<ApiKey> {
        let now = crate::db::now_unix();
        let row = sqlx::query(
            "INSERT INTO api_keys (project_id, key_lookup, key_hash, role, label, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(project_id)
        .bind(key_lookup)
        .bind(key_hash)
        .bind(role.as_str())
        .bind(label)
        .bind(now)
        .fetch_one(self.0.pool())
        .await?;

        let id: i64 = row.try_get("id")?;
        Ok(ApiKey {
            id,
            project_id,
            key_lookup: key_lookup.into(),
            key_hash: key_hash.into(),
            role,
            label: label.map(Into::into),
            created_at: now,
            revoked_at: None,
        })
    }

    /// Looks up an active key by `(project_id, key_lookup)`.
    ///
    /// Returns `None` when no matching active key exists (either absent or
    /// revoked). Callers should treat `None` as an authentication failure.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn find_active_by_lookup(
        &self,
        project_id: i64,
        key_lookup: &str,
    ) -> CoreResult<Option<ApiKey>> {
        let row = sqlx::query_as::<_, ApiKey>(
            "SELECT id, project_id, key_lookup, key_hash, role, label, created_at, revoked_at \
             FROM api_keys \
             WHERE project_id = ? AND key_lookup = ? AND revoked_at IS NULL",
        )
        .bind(project_id)
        .bind(key_lookup)
        .fetch_optional(self.0.pool())
        .await?;
        Ok(row)
    }

    /// Lists all keys for a project ordered by `id` (active and revoked).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn list(&self, project_id: i64) -> CoreResult<Vec<ApiKey>> {
        Ok(sqlx::query_as::<_, ApiKey>(
            "SELECT id, project_id, key_lookup, key_hash, role, label, created_at, revoked_at \
             FROM api_keys WHERE project_id = ? ORDER BY id",
        )
        .bind(project_id)
        .fetch_all(self.0.pool())
        .await?)
    }

    /// Marks the key identified by `id` as revoked. Idempotent is **not**
    /// guaranteed — a second call on an already-revoked key returns
    /// [`CoreError::NotFound`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when `id` does not exist or is already
    /// revoked within `project_id`.
    /// Returns [`CoreError::Database`] on any other `SQLite` failure.
    pub async fn revoke(&self, project_id: i64, id: i64) -> CoreResult<()> {
        let now = crate::db::now_unix();
        let res = sqlx::query(
            "UPDATE api_keys SET revoked_at = ? \
             WHERE project_id = ? AND id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(project_id)
        .bind(id)
        .execute(self.0.pool())
        .await?;
        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound {
                what: format!("api_key {id}"),
            });
        }
        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for ApiKey {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        let role_str: String = row.try_get("role")?;
        let role = Role::from_str(&role_str).map_err(|e| sqlx::Error::ColumnDecode {
            index: "role".into(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        })?;
        Ok(Self {
            id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            key_lookup: row.try_get("key_lookup")?,
            key_hash: row.try_get("key_hash")?,
            role,
            label: row.try_get("label")?,
            created_at: row.try_get("created_at")?,
            revoked_at: row.try_get("revoked_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a fresh in-memory database and a test project, returning both.
    async fn fresh() -> (Db, i64) {
        let db = Db::connect_memory().await.unwrap();
        let p = db.projects().create("acme", None, None).await.unwrap();
        (db, p.id)
    }

    #[tokio::test]
    async fn create_then_find() {
        let (db, pid) = fresh().await;
        db.api_keys()
            .create(pid, "lookup1", "hash1", Role::Read, Some("ci"))
            .await
            .unwrap();
        let found = db
            .api_keys()
            .find_active_by_lookup(pid, "lookup1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.role, Role::Read);
    }

    #[tokio::test]
    async fn revoke_hides_from_active_lookup() {
        let (db, pid) = fresh().await;
        let k = db
            .api_keys()
            .create(pid, "lookup1", "hash1", Role::Read, None)
            .await
            .unwrap();
        db.api_keys().revoke(pid, k.id).await.unwrap();
        assert!(
            db.api_keys()
                .find_active_by_lookup(pid, "lookup1")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn duplicate_lookup_fails() {
        let (db, pid) = fresh().await;
        db.api_keys()
            .create(pid, "lookup1", "h1", Role::Read, None)
            .await
            .unwrap();
        let e = db
            .api_keys()
            .create(pid, "lookup1", "h2", Role::Write, None)
            .await
            .unwrap_err();
        assert!(matches!(e, CoreError::Database(_)));
    }
}
