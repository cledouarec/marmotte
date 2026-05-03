//! `projects` table repository.
//

use sqlx::Row;

use crate::{
    db::Db,
    error::{CoreError, CoreResult},
    models::Project,
};

/// Project repository — CRUD operations on the `projects` table.
///
/// Obtain an instance via [`Db::projects`].
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> marmotte_core::error::CoreResult<()> {
/// use marmotte_core::Db;
/// let db = Db::connect_memory().await?;
/// let project = db.projects().create("acme", Some(1024), None).await?;
/// # Ok(())
/// # }
/// ```
pub struct Projects<'a>(pub(crate) &'a Db);

impl Projects<'_> {
    /// Inserts a new project and returns the persisted [`Project`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Conflict`] if `name` is already taken.
    /// Returns [`CoreError::Database`] on any other `SQLite` failure.
    pub async fn create(
        &self,
        name: &str,
        quota_bytes: Option<i64>,
        ttl_seconds: Option<i64>,
    ) -> CoreResult<Project> {
        let now = crate::db::now_unix();
        let row = sqlx::query(
            "INSERT INTO projects (name, quota_bytes, ttl_seconds, created_at) \
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(quota_bytes)
        .bind(ttl_seconds)
        .bind(now)
        .fetch_one(self.0.pool())
        .await
        .map_err(map_unique(name))?;

        let id: i64 = row.try_get("id")?;
        Ok(Project {
            id,
            name: name.into(),
            quota_bytes,
            ttl_seconds,
            created_at: now,
        })
    }

    /// Returns the project with the given `id`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] if no project with `id` exists.
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn get(&self, id: i64) -> CoreResult<Project> {
        let row = sqlx::query_as::<_, Project>(
            "SELECT id, name, quota_bytes, ttl_seconds, created_at FROM projects WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.0.pool())
        .await?;
        row.ok_or_else(|| CoreError::NotFound {
            what: format!("project {id}"),
        })
    }

    /// Returns the project with the given `name` (used by `Basic`-auth lookup).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] if no project named `name` exists.
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn get_by_name(&self, name: &str) -> CoreResult<Project> {
        let row = sqlx::query_as::<_, Project>(
            "SELECT id, name, quota_bytes, ttl_seconds, created_at FROM projects WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(self.0.pool())
        .await?;
        row.ok_or_else(|| CoreError::NotFound {
            what: format!("project '{name}'"),
        })
    }

    /// Lists all projects ordered by `id` (no pagination — tens, not millions).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn list(&self) -> CoreResult<Vec<Project>> {
        Ok(sqlx::query_as::<_, Project>(
            "SELECT id, name, quota_bytes, ttl_seconds, created_at FROM projects ORDER BY id",
        )
        .fetch_all(self.0.pool())
        .await?)
    }

    /// Updates an existing project's `name`, `quota_bytes`, and/or `ttl_seconds`.
    ///
    /// Each `Option` argument controls whether that field is modified:
    /// - `None` leaves the field unchanged.
    /// - `Some(v)` sets the field to `v` (for `quota_bytes` / `ttl_seconds`,
    ///   the inner `Option` is stored as-is — `Some(None)` clears the limit).
    ///
    /// Returns the updated [`Project`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Conflict`] if `new_name` is already taken.
    /// Returns [`CoreError::NotFound`] if no project with `id` exists.
    /// Returns [`CoreError::Database`] on any other `SQLite` failure.
    pub async fn update(
        &self,
        id: i64,
        new_name: Option<&str>,
        quota_bytes: Option<Option<i64>>,
        ttl_seconds: Option<Option<i64>>,
    ) -> CoreResult<Project> {
        let mut tx = self.0.pool().begin().await?;
        if let Some(name) = new_name {
            sqlx::query("UPDATE projects SET name = ? WHERE id = ?")
                .bind(name)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(map_unique(name))?;
        }
        if let Some(q) = quota_bytes {
            sqlx::query("UPDATE projects SET quota_bytes = ? WHERE id = ?")
                .bind(q)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(t) = ttl_seconds {
            sqlx::query("UPDATE projects SET ttl_seconds = ? WHERE id = ?")
                .bind(t)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.get(id).await
    }

    /// Deletes the project with `id` (cascades entries and keys).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] if no project with `id` exists.
    /// Returns [`CoreError::Database`] on a query failure.
    pub async fn delete(&self, id: i64) -> CoreResult<()> {
        let res = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(id)
            .execute(self.0.pool())
            .await?;
        if res.rows_affected() == 0 {
            return Err(CoreError::NotFound {
                what: format!("project {id}"),
            });
        }
        Ok(())
    }
}

/// Maps a `sqlx` `UNIQUE` constraint violation to [`CoreError::Conflict`].
///
/// `SQLite` surfaces `UNIQUE` violations as error code `2067`; older `sqlx`
/// versions may not expose the numeric code, so the message text is also
/// checked as a fallback.
fn map_unique(name: &str) -> impl Fn(sqlx::Error) -> CoreError + '_ {
    move |e| match &e {
        sqlx::Error::Database(db)
            if db.code().as_deref() == Some("2067") || db.message().contains("UNIQUE") =>
        {
            CoreError::Conflict {
                what: format!("project name '{name}' already exists"),
            }
        }
        _ => CoreError::Database(e),
    }
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Project {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            quota_bytes: row.try_get("quota_bytes")?,
            ttl_seconds: row.try_get("ttl_seconds")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_then_get_by_name() {
        let db = Db::connect_memory().await.unwrap();
        let p = db
            .projects()
            .create("acme", Some(1024), None)
            .await
            .unwrap();
        assert_eq!(p.name, "acme");
        let by_name = db.projects().get_by_name("acme").await.unwrap();
        assert_eq!(by_name.id, p.id);
    }

    #[tokio::test]
    async fn duplicate_name_is_conflict() {
        let db = Db::connect_memory().await.unwrap();
        db.projects().create("acme", None, None).await.unwrap();
        let e = db.projects().create("acme", None, None).await.unwrap_err();
        assert!(matches!(e, CoreError::Conflict { .. }), "got {e:?}");
    }

    #[tokio::test]
    async fn update_quota() {
        let db = Db::connect_memory().await.unwrap();
        let p = db.projects().create("acme", None, None).await.unwrap();
        let updated = db
            .projects()
            .update(p.id, None, Some(Some(2048)), None)
            .await
            .unwrap();
        assert_eq!(updated.quota_bytes, Some(2048));
    }

    #[tokio::test]
    async fn delete_unknown_returns_not_found() {
        let db = Db::connect_memory().await.unwrap();
        let e = db.projects().delete(999).await.unwrap_err();
        assert!(matches!(e, CoreError::NotFound { .. }));
    }
}
