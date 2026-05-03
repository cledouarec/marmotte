//! `blobs` table repository. Refcount is maintained transactionally by
//! `Entries::upsert` and `Entries::delete`; this module exposes the
//! lower-level primitives that those methods call.
//
// Rust guideline compliant 2026-05-06

use sqlx::{Row, SqliteExecutor};

use crate::{
    db::Db,
    error::{CoreError, CoreResult},
    models::Blob,
};

/// Blob repository — refcount-aware primitives for the `blobs` table.
///
/// Obtain an instance via [`Db::blobs`].
///
/// The two static methods [`Blobs::upsert_incr`] and [`Blobs::decr`] accept
/// any [`SqliteExecutor`] (pool reference or transaction mutable reference) so
/// callers can compose them with other statements inside a single transaction.
pub struct Blobs<'a>(pub(crate) &'a Db);

impl Blobs<'_> {
    /// Returns a single blob by `hash`, or `None` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn get(&self, hash: &str) -> CoreResult<Option<Blob>> {
        Ok(sqlx::query_as::<_, Blob>(
            "SELECT hash, size_bytes, refcount, created_at FROM blobs WHERE hash = ?",
        )
        .bind(hash)
        .fetch_optional(self.0.pool())
        .await?)
    }

    /// Inserts the blob if missing; otherwise increments its refcount by 1.
    ///
    /// Returns `true` if the blob already existed before this call, `false`
    /// if it was freshly inserted. The `executor` argument accepts either a
    /// `&SqlitePool` or a `&mut Transaction` so this call can participate in
    /// a larger transaction.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn upsert_incr<'e, E>(executor: E, hash: &str, size_bytes: i64) -> CoreResult<bool>
    where
        E: SqliteExecutor<'e>,
    {
        let now = crate::db::now_unix();
        let row = sqlx::query(
            "INSERT INTO blobs (hash, size_bytes, refcount, created_at) \
             VALUES (?, ?, 1, ?) \
             ON CONFLICT(hash) DO UPDATE SET refcount = refcount + 1 \
             RETURNING (refcount > 1) AS existed",
        )
        .bind(hash)
        .bind(size_bytes)
        .bind(now)
        .fetch_one(executor)
        .await?;
        let existed: i64 = row.try_get("existed")?;
        Ok(existed != 0)
    }

    /// Decrements refcount by 1 and returns the new refcount.
    ///
    /// The `executor` argument accepts either a `&SqlitePool` or a
    /// `&mut Transaction` so this call can participate in a larger transaction.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] if no blob with `hash` exists.
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn decr<'e, E>(executor: E, hash: &str) -> CoreResult<i64>
    where
        E: SqliteExecutor<'e>,
    {
        let row = sqlx::query(
            "UPDATE blobs SET refcount = refcount - 1 WHERE hash = ? \
             RETURNING refcount",
        )
        .bind(hash)
        .fetch_optional(executor)
        .await?;
        let row = row.ok_or_else(|| CoreError::NotFound {
            what: format!("blob {hash}"),
        })?;
        Ok(row.try_get("refcount")?)
    }

    /// Returns hashes of all blobs whose refcount has reached zero.
    ///
    /// Uses the partial index `idx_blobs_zero_refcount`. `limit` caps the
    /// result set size to avoid unbounded allocations during `GC` sweeps.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn zero_refcount(&self, limit: i64) -> CoreResult<Vec<String>> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT hash FROM blobs WHERE refcount = 0 LIMIT ?")
                .bind(limit)
                .fetch_all(self.0.pool())
                .await?,
        )
    }

    /// Deletes the blob row identified by `hash`.
    ///
    /// This is a physical delete; callers must ensure the refcount is zero
    /// (or that the backing object store object has already been removed)
    /// before calling this method.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn delete(&self, hash: &str) -> CoreResult<()> {
        sqlx::query("DELETE FROM blobs WHERE hash = ?")
            .bind(hash)
            .execute(self.0.pool())
            .await?;
        Ok(())
    }

    /// Returns the sum of `size_bytes` over all blob rows.
    ///
    /// Returns `0` when the table is empty.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn total_size(&self) -> CoreResult<i64> {
        let v: (Option<i64>,) = sqlx::query_as("SELECT SUM(size_bytes) FROM blobs")
            .fetch_one(self.0.pool())
            .await?;
        Ok(v.0.unwrap_or(0))
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Blob {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            hash: row.try_get("hash")?,
            size_bytes: row.try_get("size_bytes")?,
            refcount: row.try_get("refcount")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upsert_incr_then_decr() {
        let db = Db::connect_memory().await.unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        // First upsert: blob is new — returns false.
        assert!(!Blobs::upsert_incr(&mut *tx, "deadbeef", 100).await.unwrap());
        // Second upsert: blob already existed — returns true.
        assert!(Blobs::upsert_incr(&mut *tx, "deadbeef", 100).await.unwrap());
        // Decrement twice: refcount goes 2 → 1 → 0.
        let r = Blobs::decr(&mut *tx, "deadbeef").await.unwrap();
        assert_eq!(r, 1);
        let r = Blobs::decr(&mut *tx, "deadbeef").await.unwrap();
        assert_eq!(r, 0);
        tx.commit().await.unwrap();

        let zero = db.blobs().zero_refcount(10).await.unwrap();
        assert_eq!(zero, vec!["deadbeef".to_string()]);
    }

    #[tokio::test]
    async fn total_size_sums_existing_blobs() {
        let db = Db::connect_memory().await.unwrap();
        let mut tx = db.pool().begin().await.unwrap();
        Blobs::upsert_incr(&mut *tx, "a".repeat(64).as_str(), 100)
            .await
            .unwrap();
        Blobs::upsert_incr(&mut *tx, "b".repeat(64).as_str(), 250)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(db.blobs().total_size().await.unwrap(), 350);
    }
}
