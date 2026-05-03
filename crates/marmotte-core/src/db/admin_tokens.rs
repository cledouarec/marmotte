//! `admin_tokens` table repository.
//
// Rust guideline compliant 2026-05-06

use sqlx::Row;

use crate::{
    db::Db,
    error::{CoreError, CoreResult},
    models::AdminToken,
};

/// Admin-token repository — `CRUD` operations on the `admin_tokens` table.
///
/// Obtain an instance via [`Db::admin_tokens`].
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> marmotte_core::error::CoreResult<()> {
/// use marmotte_core::Db;
/// let db = Db::connect_memory().await?;
/// let tok = db.admin_tokens().create("sha256hex", "argon2hash", Some("root")).await?;
/// # Ok(())
/// # }
/// ```
pub struct AdminTokens<'a>(pub(crate) &'a Db);

impl AdminTokens<'_> {
    /// Inserts a new admin token and returns the persisted [`AdminToken`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on any `SQLite` failure.
    pub async fn create(
        &self,
        token_lookup: &str,
        token_hash: &str,
        label: Option<&str>,
    ) -> CoreResult<AdminToken> {
        let now = crate::db::now_unix();
        let row = sqlx::query(
            "INSERT INTO admin_tokens (token_lookup, token_hash, label, created_at) \
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(token_lookup)
        .bind(token_hash)
        .bind(label)
        .bind(now)
        .fetch_one(self.0.pool())
        .await?;
        let id: i64 = row.try_get("id")?;
        Ok(AdminToken {
            id,
            token_lookup: token_lookup.into(),
            token_hash: token_hash.into(),
            label: label.map(Into::into),
            created_at: now,
            revoked_at: None,
        })
    }

    /// Finds an active admin token by lookup hash.
    ///
    /// Returns `None` when no matching active token exists (either absent or
    /// revoked). Callers should treat `None` as an authentication failure.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn find_active_by_lookup(&self, lookup: &str) -> CoreResult<Option<AdminToken>> {
        Ok(sqlx::query_as::<_, AdminToken>(
            "SELECT id, token_lookup, token_hash, label, created_at, revoked_at \
             FROM admin_tokens WHERE token_lookup = ? AND revoked_at IS NULL",
        )
        .bind(lookup)
        .fetch_optional(self.0.pool())
        .await?)
    }

    /// Lists all admin tokens ordered by `id` (active and revoked).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn list(&self) -> CoreResult<Vec<AdminToken>> {
        Ok(sqlx::query_as::<_, AdminToken>(
            "SELECT id, token_lookup, token_hash, label, created_at, revoked_at \
             FROM admin_tokens ORDER BY id",
        )
        .fetch_all(self.0.pool())
        .await?)
    }

    /// Marks the token identified by `id` as revoked.
    ///
    /// A second call on an already-revoked token returns
    /// [`CoreError::NotFound`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when `id` does not exist or is already
    /// revoked. Returns [`CoreError::Database`] on any other `SQLite` failure.
    pub async fn revoke(&self, id: i64) -> CoreResult<()> {
        let now = crate::db::now_unix();
        let res = sqlx::query(
            "UPDATE admin_tokens SET revoked_at = ? \
             WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(self.0.pool())
        .await?;
        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound {
                what: format!("admin_token {id}"),
            });
        }
        Ok(())
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for AdminToken {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            token_lookup: row.try_get("token_lookup")?,
            token_hash: row.try_get("token_hash")?,
            label: row.try_get("label")?,
            created_at: row.try_get("created_at")?,
            revoked_at: row.try_get("revoked_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_find_revoke() {
        let db = Db::connect_memory().await.unwrap();
        let t = db
            .admin_tokens()
            .create("look", "hash", Some("root"))
            .await
            .unwrap();
        assert!(
            db.admin_tokens()
                .find_active_by_lookup("look")
                .await
                .unwrap()
                .is_some()
        );
        db.admin_tokens().revoke(t.id).await.unwrap();
        assert!(
            db.admin_tokens()
                .find_active_by_lookup("look")
                .await
                .unwrap()
                .is_none()
        );
    }
}
