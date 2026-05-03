//! Stats counters and audit log.
//
// Rust guideline compliant 2026-05-06

use sqlx::Row;

use crate::{db::Db, error::CoreResult};

/// Stats / audit repository.
///
/// Provides two surfaces:
///
/// - **Counters** — day-bucketed `(project_id, kind)` pairs that accumulate
///   event counts and byte totals with upsert semantics.
/// - **Audit log** — append-only, timestamped record of named actions with an
///   optional actor, target, and structured detail payload.
///
/// Obtain an instance via [`Db::stats`].
pub struct Stats<'a>(pub(crate) &'a Db);

impl Stats<'_> {
    /// Bumps `(project_id, kind, today)` counter by `count` events and `bytes` total.
    ///
    /// The day bucket is derived from the current Unix timestamp divided by
    /// `86 400` (seconds per day). Repeated calls on the same calendar day
    /// accumulate into the same row via `ON CONFLICT … DO UPDATE`.
    ///
    /// `project_id` may be `None` to record a global (cross-project) counter.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Database`] on a query failure.
    pub async fn bump(
        &self,
        project_id: Option<i64>,
        kind: &str,
        count: i64,
        bytes: i64,
    ) -> CoreResult<()> {
        // 86 400 = seconds per day; dividing the Unix timestamp gives the ordinal day.
        let bucket = crate::db::now_unix() / 86_400;
        sqlx::query(
            "INSERT INTO stats_counters (project_id, kind, bucket_day, count, bytes) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(project_id, kind, bucket_day) \
             DO UPDATE SET count = count + excluded.count, bytes = bytes + excluded.bytes",
        )
        .bind(project_id)
        .bind(kind)
        .bind(bucket)
        .bind(count)
        .bind(bytes)
        .execute(self.0.pool())
        .await?;
        Ok(())
    }

    /// Sums `(count, bytes)` over `kind` for a project (or globally) since `since_day`.
    ///
    /// `since_day` is an inclusive lower bound expressed as an ordinal day number
    /// (i.e., `unix_ts / 86_400`). Pass `0` to aggregate all history.
    ///
    /// When `project_id` is `None` the query covers all projects.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Database`] on a query failure.
    pub async fn sum(
        &self,
        project_id: Option<i64>,
        kind: &str,
        since_day: i64,
    ) -> CoreResult<(i64, i64)> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(count),0) AS c, COALESCE(SUM(bytes),0) AS b \
             FROM stats_counters \
             WHERE kind = ? AND bucket_day >= ? \
               AND (? IS NULL OR project_id = ?)",
        )
        .bind(kind)
        .bind(since_day)
        .bind(project_id)
        .bind(project_id)
        .fetch_one(self.0.pool())
        .await?;
        Ok((row.try_get("c")?, row.try_get("b")?))
    }

    /// Appends an audit-log row.
    ///
    /// All parameters other than `action` are optional:
    ///
    /// - `actor_token` — opaque identifier for the calling principal.
    /// - `target` — resource affected by the action (e.g., a project name).
    /// - `detail` — arbitrary structured context serialised as `JSON`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Database`] on a query failure.
    pub async fn audit(
        &self,
        actor_token: Option<&str>,
        action: &str,
        target: Option<&str>,
        detail: Option<&serde_json::Value>,
    ) -> CoreResult<()> {
        let now = crate::db::now_unix();
        let detail_str = detail.map(std::string::ToString::to_string);
        sqlx::query(
            "INSERT INTO audit_log (timestamp, actor_token, action, target, detail_json) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(now)
        .bind(actor_token)
        .bind(action)
        .bind(target)
        .bind(detail_str)
        .execute(self.0.pool())
        .await?;
        Ok(())
    }

    /// Reads audit-log rows since `since_ts`, newest first, up to `limit`.
    ///
    /// Returns each row as a [`serde_json::Value`] object with the fields
    /// `id`, `timestamp`, `actor_token`, `action`, `target`, and `detail`.
    ///
    /// Per-row decode errors are swallowed and replaced with zero/empty
    /// defaults rather than failing the entire call. The audit surface is
    /// informational; partial row corruption must not propagate as a 500.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Database`] on a query failure.
    pub async fn audit_since(
        &self,
        since_ts: i64,
        limit: i64,
    ) -> CoreResult<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            "SELECT id, timestamp, actor_token, action, target, detail_json \
             FROM audit_log WHERE timestamp >= ? \
             ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(since_ts)
        .bind(limit)
        .fetch_all(self.0.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.try_get::<i64, _>("id").unwrap_or(0),
                    "timestamp": r.try_get::<i64, _>("timestamp").unwrap_or(0),
                    "actor_token": r.try_get::<Option<String>, _>("actor_token").unwrap_or_default(),
                    "action": r.try_get::<String, _>("action").unwrap_or_default(),
                    "target": r.try_get::<Option<String>, _>("target").unwrap_or_default(),
                    "detail": r.try_get::<Option<String>, _>("detail_json").ok().flatten()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bump_then_sum() {
        let db = Db::connect_memory().await.unwrap();
        db.stats().bump(None, "hit", 3, 1000).await.unwrap();
        db.stats().bump(None, "hit", 2, 500).await.unwrap();
        let (c, b) = db.stats().sum(None, "hit", 0).await.unwrap();
        assert_eq!((c, b), (5, 1500));
    }

    #[tokio::test]
    async fn audit_round_trip() {
        let db = Db::connect_memory().await.unwrap();
        db.stats()
            .audit(
                Some("root"),
                "project.create",
                Some("acme"),
                Some(&serde_json::json!({"id": 1})),
            )
            .await
            .unwrap();
        let rows = db.stats().audit_since(0, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["action"], "project.create");
    }
}
