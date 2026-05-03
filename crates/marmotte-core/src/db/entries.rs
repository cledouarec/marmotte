//! `entries` table repository: upsert (with refcount), delete, list with
//! cursor pagination.
//

use std::fmt::Write as _;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{
    db::{Db, blobs::Blobs},
    error::{CoreError, CoreResult},
    models::{Entry, Kind},
};

/// Sort field for entry listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortField {
    /// Sort by the entry path string.
    Path,
    /// Sort by the time the entry was last accessed.
    LastAccessed,
    /// Sort by the blob size in bytes.
    Size,
    /// Sort by the entry creation time (by surrogate `id`).
    CreatedAt,
}

impl SortField {
    /// Returns the `SQL` column name to use in `ORDER BY`.
    fn column(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::LastAccessed => "last_accessed",
            Self::Size => "size_bytes",
            Self::CreatedAt => "id",
        }
    }
}

/// Sort order for entry listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    /// Ascending order (smallest / earliest first).
    Asc,
    /// Descending order (largest / latest first).
    Desc,
}

/// Filters, cursor, and sort parameters for [`Entries::list`].
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// Restrict results to a specific cache namespace.
    pub kind: Option<Kind>,
    /// Restrict results to pinned or unpinned entries.
    pub pinned: Option<bool>,
    /// Restrict results to entries whose path starts with this prefix.
    pub path_prefix: Option<String>,
    /// Field to sort by; defaults to [`SortField::LastAccessed`].
    pub sort: Option<SortField>,
    /// Sort direction; defaults to [`SortOrder::Desc`].
    pub order: Option<SortOrder>,
    /// Maximum number of entries per page (1–1000, default 100).
    pub limit: Option<i64>,
    /// Opaque cursor returned by a previous call to [`Entries::list`].
    pub cursor: Option<String>,
}

/// A page of entries returned by [`Entries::list`].
#[derive(Debug, Clone)]
pub struct Page {
    /// Entries on this page (at most `limit` items).
    pub entries: Vec<Entry>,
    /// Opaque cursor; pass as `ListFilter::cursor` to retrieve the next page.
    pub next_cursor: Option<String>,
    /// `true` when at least one more page exists after this one.
    pub has_more: bool,
}

/// Cursor payload (opaque on the wire, `JSON` + base64url-no-pad).
#[derive(Debug, Serialize, Deserialize)]
struct Cursor {
    /// The sort-field value of the last entry on the page.
    sv: serde_json::Value,
    /// The surrogate `id` of the last entry on the page (tie-breaker).
    id: i64,
}

impl Cursor {
    /// Encodes the cursor to a base64url-no-pad string.
    fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("serialize cursor"))
    }

    /// Decodes a cursor from a base64url-no-pad string.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidInput`] if the string is not valid base64 or
    /// the resulting bytes do not deserialize as a [`Cursor`].
    fn decode(s: &str) -> CoreResult<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|e| CoreError::InvalidInput(format!("bad cursor: {e}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::InvalidInput(format!("bad cursor: {e}")))
    }
}

/// Entry repository — `(project, kind, path) → blob` mapping with refcount.
///
/// Obtain an instance via [`Db::entries`].
///
/// Every mutation that changes blob references (`upsert`, `delete`) uses a
/// single `SQLite` transaction so the refcount in the `blobs` table is always
/// consistent with the `entries` table.
pub struct Entries<'a>(pub(crate) &'a Db);

impl Entries<'_> {
    /// Inserts or replaces an entry, maintaining blob refcounts atomically.
    ///
    /// If an entry already exists for `(project_id, kind, path)` and its
    /// `blob_hash` differs from `new_blob_hash`, the old blob is decremented
    /// and the new one incremented within the same transaction. If the blob
    /// hash is unchanged, refcounts are not touched.
    ///
    /// `size_bytes` is the blob size (single source of truth; the `entries`
    /// column is a denormalized cache of `blobs.size_bytes`).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on any `SQLite` failure.
    pub async fn upsert(
        &self,
        project_id: i64,
        kind: Kind,
        path: &str,
        new_blob_hash: &str,
        size_bytes: i64,
    ) -> CoreResult<Entry> {
        let mut tx = self.0.pool().begin().await?;

        // Check whether the entry already exists and what blob it points to.
        let prev: Option<(i64, String)> = sqlx::query_as(
            "SELECT id, blob_hash FROM entries \
             WHERE project_id = ? AND kind = ? AND path = ?",
        )
        .bind(project_id)
        .bind(kind.as_str())
        .bind(path)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((_, ref old_hash)) = prev {
            // Entry exists: only touch refcounts when the blob actually changes.
            if old_hash != new_blob_hash {
                Blobs::decr(&mut *tx, old_hash).await?;
                Blobs::upsert_incr(&mut *tx, new_blob_hash, size_bytes).await?;
            }
        } else {
            // New entry: unconditionally increment the new blob's refcount.
            Blobs::upsert_incr(&mut *tx, new_blob_hash, size_bytes).await?;
        }

        let now = crate::db::now_unix();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO entries \
                 (project_id, kind, path, blob_hash, size_bytes, created_at, last_accessed, pinned) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 0) \
             ON CONFLICT(project_id, kind, path) DO UPDATE SET \
                 blob_hash = excluded.blob_hash, \
                 size_bytes = excluded.size_bytes, \
                 last_accessed = excluded.last_accessed \
             RETURNING id",
        )
        .bind(project_id)
        .bind(kind.as_str())
        .bind(path)
        .bind(new_blob_hash)
        .bind(size_bytes)
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Entry {
            id,
            project_id,
            kind,
            path: path.into(),
            blob_hash: new_blob_hash.into(),
            size_bytes,
            created_at: now,
            last_accessed: now,
            pinned: false,
        })
    }

    /// Returns an entry by `(project_id, kind, path)`, or `None` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn find(&self, project_id: i64, kind: Kind, path: &str) -> CoreResult<Option<Entry>> {
        Ok(sqlx::query_as::<_, Entry>(
            "SELECT id, project_id, kind, path, blob_hash, size_bytes, \
                    created_at, last_accessed, pinned \
             FROM entries WHERE project_id = ? AND kind = ? AND path = ?",
        )
        .bind(project_id)
        .bind(kind.as_str())
        .bind(path)
        .fetch_optional(self.0.pool())
        .await?)
    }

    /// Best-effort `last_accessed` timestamp bump.
    ///
    /// Errors are logged at `WARN` level and swallowed so callers do not fail
    /// a `GET` just because the access-time update failed.
    pub async fn touch(&self, id: i64) {
        let now = crate::db::now_unix();
        if let Err(e) = sqlx::query("UPDATE entries SET last_accessed = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(self.0.pool())
            .await
        {
            tracing::warn!(error = %e, entry_id = id, "failed to update last_accessed");
        }
    }

    /// Returns the total logical storage used by a project (sum of `size_bytes`).
    ///
    /// Returns `0` when the project has no entries.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn project_usage(&self, project_id: i64) -> CoreResult<i64> {
        let v: (Option<i64>,) =
            sqlx::query_as("SELECT SUM(size_bytes) FROM entries WHERE project_id = ?")
                .bind(project_id)
                .fetch_one(self.0.pool())
                .await?;
        Ok(v.0.unwrap_or(0))
    }

    /// Deletes an entry by `id`, decrementing the underlying blob refcount.
    ///
    /// Both the `DELETE` and the refcount decrement are performed inside a
    /// single transaction.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] if no entry with `(project_id, id)`
    /// exists. Returns [`CoreError::Database`] on any other `SQLite` failure.
    pub async fn delete(&self, project_id: i64, id: i64) -> CoreResult<()> {
        let mut tx = self.0.pool().begin().await?;
        let row: Option<String> =
            sqlx::query_scalar("SELECT blob_hash FROM entries WHERE project_id = ? AND id = ?")
                .bind(project_id)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let hash = row.ok_or_else(|| CoreError::NotFound {
            what: format!("entry {id}"),
        })?;
        sqlx::query("DELETE FROM entries WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        Blobs::decr(&mut *tx, &hash).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Sets the `pinned` flag on an entry.
    ///
    /// Pinned entries are never evicted by the `GC` engine.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] if no entry with `(project_id, id)`
    /// exists. Returns [`CoreError::Database`] on any other `SQLite` failure.
    pub async fn set_pinned(&self, project_id: i64, id: i64, pinned: bool) -> CoreResult<()> {
        let res = sqlx::query("UPDATE entries SET pinned = ? WHERE project_id = ? AND id = ?")
            .bind(i64::from(pinned))
            .bind(project_id)
            .bind(id)
            .execute(self.0.pool())
            .await?;
        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound {
                what: format!("entry {id}"),
            });
        }
        Ok(())
    }

    /// Returns a cursor-paginated page of entries for a project.
    ///
    /// `path_prefix` uses `LIKE 'prefix%'`; all other filters are exact. The
    /// cursor is opaque on the wire (`JSON` + base64url-no-pad); pass the
    /// `next_cursor` from a [`Page`] back as `ListFilter::cursor` to advance.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidInput`] if `cursor` cannot be decoded.
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn list(&self, project_id: i64, f: ListFilter) -> CoreResult<Page> {
        let sort = f.sort.unwrap_or(SortField::LastAccessed);
        let order = f.order.unwrap_or(SortOrder::Desc);
        let limit = f.limit.unwrap_or(100).clamp(1, 1000);
        let order_sql = match order {
            SortOrder::Asc => "ASC",
            SortOrder::Desc => "DESC",
        };
        let cmp_sql = match order {
            SortOrder::Asc => ">",
            SortOrder::Desc => "<",
        };

        let mut sql = String::from(
            "SELECT id, project_id, kind, path, blob_hash, size_bytes, \
                    created_at, last_accessed, pinned \
             FROM entries WHERE project_id = ?",
        );
        let mut binds: Vec<Bind> = vec![Bind::I64(project_id)];

        if let Some(k) = f.kind {
            sql.push_str(" AND kind = ?");
            binds.push(Bind::Str(k.as_str().into()));
        }
        if let Some(p) = f.pinned {
            sql.push_str(" AND pinned = ?");
            binds.push(Bind::I64(i64::from(p)));
        }
        if let Some(prefix) = &f.path_prefix {
            sql.push_str(" AND path LIKE ?");
            binds.push(Bind::Str(format!("{prefix}%")));
        }
        if let Some(c) = &f.cursor {
            let cur = Cursor::decode(c)?;
            write!(
                sql,
                " AND ({col} {cmp} ? OR ({col} = ? AND id {cmp} ?))",
                col = sort.column(),
                cmp = cmp_sql,
            )
            .expect("write to String is infallible");
            binds.push(Bind::Json(cur.sv.clone()));
            binds.push(Bind::Json(cur.sv));
            binds.push(Bind::I64(cur.id));
        }
        write!(
            sql,
            " ORDER BY {col} {ord}, id {ord} LIMIT ?",
            col = sort.column(),
            ord = order_sql,
        )
        .expect("write to String is infallible");
        // Fetch one extra row to determine whether another page exists.
        binds.push(Bind::I64(limit + 1));

        let mut q = sqlx::query_as::<_, Entry>(&sql);
        for b in &binds {
            q = match b {
                Bind::I64(v) => q.bind(*v),
                Bind::Str(v) => q.bind(v.clone()),
                Bind::Json(v) => bind_json(q, v),
            };
        }
        let mut rows: Vec<Entry> = q.fetch_all(self.0.pool()).await?;

        // `limit` is clamped to 1..=1000; the value is always non-negative so
        // casting to `usize` cannot lose the sign in practice.
        #[expect(
            clippy::cast_sign_loss,
            reason = "limit clamped to 1..=1000 by clamp() above; value is always positive"
        )]
        let limit_usize = limit as usize;

        // `rows.len()` is at most `limit + 1` ≤ 1001, which fits in both
        // `usize` and `i64` on every supported target.
        #[expect(
            clippy::cast_possible_wrap,
            reason = "rows.len() <= limit+1 <= 1001; cannot wrap into a negative i64"
        )]
        let has_more = rows.len() as i64 > limit;

        if has_more {
            rows.truncate(limit_usize);
        }

        let next_cursor = if has_more {
            rows.last().map(|e| {
                Cursor {
                    sv: sort_value_of(e, sort),
                    id: e.id,
                }
                .encode()
            })
        } else {
            None
        };

        Ok(Page {
            entries: rows,
            next_cursor,
            has_more,
        })
    }
}

/// Bound parameter variant for the dynamic `SQL` builder in [`Entries::list`].
///
/// Values are kept alive for the entire duration of the binding loop so the
/// `sqlx` query object can borrow them by reference.
enum Bind {
    /// Integer parameter.
    I64(i64),
    /// String parameter.
    Str(String),
    /// `JSON` value: bound as its native type (integer, float, or string).
    Json(serde_json::Value),
}

/// Binds a [`serde_json::Value`] to a `sqlx` query by its runtime type.
///
/// Integer values are bound as `i64`; other numbers as `f64`; strings as
/// `String`; everything else falls back to the `JSON` text representation.
fn bind_json<'q, O>(
    q: sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>>,
    v: &serde_json::Value,
) -> sqlx::query::QueryAs<'q, sqlx::Sqlite, O, sqlx::sqlite::SqliteArguments<'q>> {
    match v {
        serde_json::Value::Number(n) if n.is_i64() => {
            q.bind(n.as_i64().expect("is_i64 guard passed"))
        }
        serde_json::Value::Number(n) => q.bind(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => q.bind(s.clone()),
        _ => q.bind(v.to_string()),
    }
}

/// Extracts the sort-field value of `e` as a [`serde_json::Value`].
///
/// Used to build the cursor for the last entry on a page.
fn sort_value_of(e: &Entry, s: SortField) -> serde_json::Value {
    match s {
        SortField::Path => serde_json::Value::String(e.path.clone()),
        SortField::LastAccessed => serde_json::Value::from(e.last_accessed),
        SortField::Size => serde_json::Value::from(e.size_bytes),
        SortField::CreatedAt => serde_json::Value::from(e.id),
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Entry {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        let kind_str: String = row.try_get("kind")?;
        let kind = Kind::from_str(&kind_str).map_err(|e| sqlx::Error::ColumnDecode {
            index: "kind".into(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        })?;
        let pinned_i: i64 = row.try_get("pinned")?;
        Ok(Self {
            id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            kind,
            path: row.try_get("path")?,
            blob_hash: row.try_get("blob_hash")?,
            size_bytes: row.try_get("size_bytes")?,
            created_at: row.try_get("created_at")?,
            last_accessed: row.try_get("last_accessed")?,
            pinned: pinned_i != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a fresh in-memory database with one project.
    async fn fresh() -> (Db, i64) {
        let db = Db::connect_memory().await.unwrap();
        let p = db.projects().create("acme", None, None).await.unwrap();
        (db, p.id)
    }

    #[tokio::test]
    async fn upsert_increments_refcount_when_blob_is_new() {
        let (db, pid) = fresh().await;
        let h = "a".repeat(64);
        db.entries()
            .upsert(pid, Kind::Sstate, "foo", &h, 10)
            .await
            .unwrap();
        assert_eq!(db.blobs().get(&h).await.unwrap().unwrap().refcount, 1);
    }

    #[tokio::test]
    async fn upsert_replaces_blob_decrements_old() {
        let (db, pid) = fresh().await;
        let h1 = "a".repeat(64);
        let h2 = "b".repeat(64);
        db.entries()
            .upsert(pid, Kind::Sstate, "foo", &h1, 10)
            .await
            .unwrap();
        db.entries()
            .upsert(pid, Kind::Sstate, "foo", &h2, 20)
            .await
            .unwrap();
        assert_eq!(db.blobs().get(&h1).await.unwrap().unwrap().refcount, 0);
        assert_eq!(db.blobs().get(&h2).await.unwrap().unwrap().refcount, 1);
    }

    #[tokio::test]
    async fn delete_decrements_refcount() {
        let (db, pid) = fresh().await;
        let h = "a".repeat(64);
        let e = db
            .entries()
            .upsert(pid, Kind::Sstate, "foo", &h, 10)
            .await
            .unwrap();
        db.entries().delete(pid, e.id).await.unwrap();
        assert_eq!(db.blobs().get(&h).await.unwrap().unwrap().refcount, 0);
    }

    #[tokio::test]
    async fn pagination_traverses_all_with_cursor() {
        let (db, pid) = fresh().await;
        for i in 0..5 {
            let h = format!("{i:0>64}");
            db.entries()
                .upsert(pid, Kind::Sstate, &format!("p/{i}"), &h, 10)
                .await
                .unwrap();
        }
        let mut got = vec![];
        let mut filter = ListFilter {
            limit: Some(2),
            sort: Some(SortField::Path),
            order: Some(SortOrder::Asc),
            ..Default::default()
        };
        loop {
            let page = db.entries().list(pid, filter.clone()).await.unwrap();
            for e in &page.entries {
                got.push(e.path.clone());
            }
            match page.next_cursor {
                Some(c) => filter.cursor = Some(c),
                None => break,
            }
        }
        assert_eq!(got, vec!["p/0", "p/1", "p/2", "p/3", "p/4"]);
    }

    #[tokio::test]
    async fn pin_excludes_from_pinned_filter() {
        let (db, pid) = fresh().await;
        let h = "a".repeat(64);
        let e = db
            .entries()
            .upsert(pid, Kind::Sstate, "foo", &h, 10)
            .await
            .unwrap();
        db.entries().set_pinned(pid, e.id, true).await.unwrap();
        let page = db
            .entries()
            .list(
                pid,
                ListFilter {
                    pinned: Some(false),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(page.entries.is_empty());
    }
}
