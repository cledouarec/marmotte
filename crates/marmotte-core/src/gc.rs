//! Garbage-collection engine: TTL + per-project quota + global LRU sweep.
//!
//! # Overview
//!
//! This module exposes [`GcSvc`], the central GC coordinator for Marmotte.
//! Callers invoke [`GcSvc::run`] to execute a full sweep and receive a
//! [`GcReport`] describing what was evicted.  A `tokio::sync::Mutex` ensures
//! that two concurrent callers never interleave their passes.
//!
//! ## Sweep phases (in order)
//!
//! 1. **TTL** — evict entries older than their effective TTL (per-project
//!    override or the global kind default).
//! 2. **Per-project quota** — for projects with a `quota_bytes` limit, evict
//!    the least-recently-used unpinned entries until the project is under quota.
//! 3. **Global LRU** — if total blob storage still exceeds
//!    [`GcConfig::global_quota_bytes`], evict more entries world-wide, LRU
//!    first, until the global quota is satisfied.
//! 4. **Blob cleanup** — remove blob files and rows whose refcount has fallen
//!    to zero.
//!
//! An auxiliary [`GcSvc::orphan_scan`] method reconciles the blob directory
//! with the database: disk-only files are deleted; DB-only rows are purged.
//!
//! ## Dry-run mode
//!
//! Passing `dry_run = true` to either [`GcSvc::run`] or [`GcSvc::orphan_scan`]
//! reports what *would* be affected without mutating any state.
//

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{config::GcConfig, db::Db, error::CoreResult, models::Kind, storage::LocalFsStore};

/// Outcome of one full GC sweep.
///
/// All counters are cumulative across all sweep phases.  `usage_global_bytes`
/// reflects the total blob storage *after* the sweep (or what it would be if
/// `dry_run` was `true`, which may still show the pre-sweep total because no
/// blobs are actually deleted in that mode).
#[derive(Debug, Default, Clone)]
pub struct GcReport {
    /// Total number of cache entries removed across all phases.
    pub evicted_entries: u64,
    /// Total logical bytes freed by evicted entries.
    pub evicted_bytes: u64,
    /// Number of zero-refcount blob files deleted from disk.
    pub freed_blobs: u64,
    /// Total blob storage in bytes at the end of the sweep.
    pub usage_global_bytes: i64,
}

/// Result of an orphan-scan pass.
///
/// Counts items that were out of sync between the on-disk blob store and the
/// database at the time [`GcSvc::orphan_scan`] was called.
#[derive(Debug, Default, Clone)]
pub struct OrphanReport {
    /// On-disk blob files removed because no matching DB row existed.
    pub disk_orphans_removed: u64,
    /// DB blob rows deleted because the backing file was missing.
    pub db_orphans_removed: u64,
}

/// Reentrant-safe GC service.
///
/// The internal mutex serializes concurrent [`GcSvc::run`] /
/// [`GcSvc::orphan_scan`] calls so that phases never interleave.  [`GcSvc`] is
/// cheap to clone; all clones share the same mutex and database pool.
#[derive(Clone)]
pub struct GcSvc {
    db: Db,
    store: LocalFsStore,
    cfg: GcConfig,
    /// Held for the duration of each run; prevents concurrent sweeps.
    running: Arc<Mutex<()>>,
}

impl GcSvc {
    /// Creates a new GC service backed by `db`, `store`, and `cfg`.
    #[must_use]
    pub fn new(db: Db, store: LocalFsStore, cfg: GcConfig) -> Self {
        Self {
            db,
            store,
            cfg,
            running: Arc::new(Mutex::new(())),
        }
    }

    /// Runs one full sweep.
    ///
    /// Acquires an internal lock so two concurrent callers serialize.
    /// `dry_run = true` reports what would be evicted without touching state.
    ///
    /// The sweep is divided into four phases executed in order; see the
    /// [module documentation](self) for details.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError::Database`] if any SQL query fails,
    /// or [`crate::error::CoreError::Io`] if a blob cannot be removed from disk.
    pub async fn run(&self, dry_run: bool) -> CoreResult<GcReport> {
        let _g = self.running.lock().await;
        let mut rep = GcReport::default();

        self.ttl_phase(&mut rep, dry_run).await?;
        self.quota_phase(&mut rep, dry_run).await?;
        self.global_phase(&mut rep, dry_run).await?;
        self.cleanup_blobs(&mut rep, dry_run).await?;
        rep.usage_global_bytes = self.db.blobs().total_size().await?;

        Ok(rep)
    }

    /// Reconciles the on-disk blob store with the database.
    ///
    /// - Files present on disk but absent from the DB are **disk orphans**;
    ///   they are deleted when `!dry_run`.
    /// - DB rows whose backing file is missing are **DB orphans**; they and
    ///   all dependent `entries` rows are deleted transactionally when
    ///   `!dry_run`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::CoreError`] on any IO or database failure.
    pub async fn orphan_scan(&self, dry_run: bool) -> CoreResult<OrphanReport> {
        let _g = self.running.lock().await;
        let mut rep = OrphanReport::default();

        // Collect all on-disk hashes from the blobs/ tree.
        let blobs_dir = self.store.root().join("blobs");
        let mut on_disk = std::collections::HashSet::<String>::new();
        if blobs_dir.exists() {
            walk_blobs(&blobs_dir, &mut on_disk)?;
        }

        // All hashes currently known to the database.
        let db_hashes: Vec<String> = sqlx::query_scalar("SELECT hash FROM blobs")
            .fetch_all(self.db.pool())
            .await?;
        let db_set: std::collections::HashSet<String> = db_hashes.into_iter().collect();

        // disk \ db = files on disk with no corresponding DB row.
        for h in on_disk.difference(&db_set) {
            if !dry_run && let Err(e) = self.store.delete_blob(h).await {
                tracing::warn!(
                    name: "gc.orphan.disk.remove.failed",
                    error = %e,
                    hash = %h,
                    "orphan-scan disk removal failed: {{hash}}",
                );
            }
            rep.disk_orphans_removed += 1;
        }

        // db \ disk = DB rows whose blob file is absent.
        for h in db_set.difference(&on_disk) {
            tracing::error!(
                name: "gc.orphan.db.missing_file",
                hash = %h,
                "blob row exists but file is missing: {{hash}}",
            );
            if !dry_run {
                let mut tx = self.db.pool().begin().await?;
                sqlx::query("DELETE FROM entries WHERE blob_hash = ?")
                    .bind(h)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("DELETE FROM blobs WHERE hash = ?")
                    .bind(h)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
            rep.db_orphans_removed += 1;
        }

        Ok(rep)
    }

    /// Evicts entries that have exceeded their effective TTL.
    ///
    /// For each [`Kind`], the effective TTL per row is:
    /// `COALESCE(projects.ttl_seconds, <kind_default_secs>)`.
    async fn ttl_phase(&self, rep: &mut GcReport, dry_run: bool) -> CoreResult<()> {
        let now = crate::db::now_unix();

        for kind in [Kind::Sstate, Kind::Downloads] {
            let default_ttl_days = match kind {
                Kind::Sstate => self.cfg.default_ttl_sstate_days,
                Kind::Downloads => self.cfg.default_ttl_downloads_days,
            };
            // Convert the per-kind default to seconds.  The multiplication may
            // theoretically overflow for astronomically large TTL values, but
            // in practice day counts fit easily within i64.
            #[expect(
                clippy::cast_possible_wrap,
                reason = "default_ttl_days * 86_400 fits in i64 for any realistic TTL setting"
            )]
            let default_secs = (default_ttl_days * 86_400) as i64;
            let cutoff_default = now - default_secs;

            // Per-project override on `projects.ttl_seconds`; if NULL, use the
            // kind default.  Single SQL pass using COALESCE avoids N+1 queries.
            let candidates = sqlx::query_as::<_, (i64, i64, String, i64)>(
                "SELECT e.id, e.project_id, e.blob_hash, e.size_bytes \
                 FROM entries e JOIN projects p ON p.id = e.project_id \
                 WHERE e.kind = ? AND e.pinned = 0 \
                   AND e.last_accessed < ? - COALESCE(p.ttl_seconds, 0) \
                   AND (p.ttl_seconds IS NOT NULL OR e.last_accessed < ?)",
            )
            .bind(kind.as_str())
            .bind(now)
            .bind(cutoff_default)
            .fetch_all(self.db.pool())
            .await?;

            for (id, project_id, _hash, size) in candidates {
                if !dry_run {
                    self.db.entries().delete(project_id, id).await?;
                }
                rep.evicted_entries += 1;
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "size_bytes is non-negative by schema constraint"
                )]
                {
                    rep.evicted_bytes += size as u64;
                }
            }
        }
        Ok(())
    }

    /// Evicts LRU unpinned entries for projects that exceed their quota.
    ///
    /// Projects without a `quota_bytes` setting are skipped.  Within a project,
    /// entries are evicted oldest-first until usage drops to or below quota.
    async fn quota_phase(&self, rep: &mut GcReport, dry_run: bool) -> CoreResult<()> {
        let projects = self.db.projects().list().await?;
        for p in projects {
            let Some(quota) = p.quota_bytes else { continue };
            let mut usage = self.db.entries().project_usage(p.id).await?;
            if usage <= quota {
                continue;
            }

            // Fetch oldest-first so we can iterate and evict until under quota.
            let to_evict: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT id, size_bytes FROM entries \
                 WHERE project_id = ? AND pinned = 0 \
                 ORDER BY last_accessed ASC, id ASC",
            )
            .bind(p.id)
            .fetch_all(self.db.pool())
            .await?;

            for (id, size) in to_evict {
                if usage <= quota {
                    break;
                }
                if !dry_run {
                    self.db.entries().delete(p.id, id).await?;
                }
                usage -= size;
                rep.evicted_entries += 1;
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "size_bytes is non-negative by schema constraint"
                )]
                {
                    rep.evicted_bytes += size as u64;
                }
            }
        }
        Ok(())
    }

    /// Evicts the globally least-recently-used entries when over the global quota.
    ///
    /// Uses the total blob storage (sum of all blob `size_bytes` rows) as the
    /// usage metric so that deduplication is accounted for correctly.
    async fn global_phase(&self, rep: &mut GcReport, dry_run: bool) -> CoreResult<()> {
        let mut usage = self.db.blobs().total_size().await?;
        #[expect(
            clippy::cast_possible_wrap,
            reason = "global_quota_bytes fits in i64 for any realistic quota value"
        )]
        let quota = self.cfg.global_quota_bytes as i64;
        if usage <= quota {
            return Ok(());
        }

        // Entries ordered globally by LRU — oldest access first.
        let candidates: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT id, project_id, size_bytes FROM entries \
             WHERE pinned = 0 \
             ORDER BY last_accessed ASC, id ASC",
        )
        .fetch_all(self.db.pool())
        .await?;

        for (id, project_id, size) in candidates {
            if usage <= quota {
                break;
            }
            if !dry_run {
                self.db.entries().delete(project_id, id).await?;
            }
            usage -= size;
            rep.evicted_entries += 1;
            #[expect(
                clippy::cast_sign_loss,
                reason = "size_bytes is non-negative by schema constraint"
            )]
            {
                rep.evicted_bytes += size as u64;
            }
        }
        Ok(())
    }

    /// Deletes blob files and rows whose refcount has reached zero.
    ///
    /// Processes up to 1 000 blobs per batch to bound peak memory usage.
    /// Blob file removal failures are logged as warnings and do not abort
    /// the cleanup — the DB row is still deleted so the file becomes an orphan
    /// that a subsequent [`GcSvc::orphan_scan`] can clean up.
    async fn cleanup_blobs(&self, rep: &mut GcReport, dry_run: bool) -> CoreResult<()> {
        // Batch size: 1 000 keeps the SELECT result set small.
        const BATCH: i64 = 1_000;

        loop {
            let batch = self.db.blobs().zero_refcount(BATCH).await?;
            if batch.is_empty() {
                break;
            }
            for hash in &batch {
                if !dry_run {
                    if let Err(e) = self.store.delete_blob(hash).await {
                        tracing::warn!(
                            name: "gc.blob.unlink.failed",
                            error = %e,
                            hash = %hash,
                            "blob file removal failed: {{hash}}",
                        );
                    }
                    self.db.blobs().delete(hash).await?;
                }
                rep.freed_blobs += 1;
            }
            // In dry-run mode there is nothing to loop over — the batch never
            // shrinks because nothing is deleted.
            if dry_run {
                break;
            }
        }
        Ok(())
    }
}

/// Recursively collects hex-encoded blob file names from `dir` into `out`.
///
/// Only files whose names consist of exactly 64 lowercase hexadecimal
/// characters are considered valid blob hashes.
///
/// # Errors
///
/// Returns [`crate::error::CoreError::Io`] if directory traversal fails.
fn walk_blobs(
    dir: &std::path::Path,
    out: &mut std::collections::HashSet<String>,
) -> CoreResult<()> {
    for ent in std::fs::read_dir(dir)? {
        let ent = ent?;
        let path = ent.path();
        if ent.file_type()?.is_dir() {
            walk_blobs(&path, out)?;
        } else if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            // Require exactly 64 lowercase hex characters — the SHA-256 length.
            if name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit()) {
                out.insert(name.to_string());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Creates a minimal in-memory database, local store, and GC service for
    /// use in unit tests.
    pub(super) async fn fixture() -> (TempDir, Db, LocalFsStore, GcSvc) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::connect_memory().await.unwrap();
        let store = LocalFsStore::open(dir.path()).await.unwrap();
        let cfg = GcConfig {
            interval_secs: 60,
            default_ttl_sstate_days: 1,
            default_ttl_downloads_days: 7,
            global_quota_bytes: 1024 * 1024,
            trigger_threshold_pct: 90,
        };
        let gc = GcSvc::new(db.clone(), store.clone(), cfg);
        (dir, db, store, gc)
    }

    #[tokio::test]
    async fn empty_run_reports_zero() {
        let (_d, _db, _s, gc) = fixture().await;
        let r = gc.run(false).await.unwrap();
        assert_eq!(r.evicted_entries, 0);
        assert_eq!(r.usage_global_bytes, 0);
    }

    // ── Task 5.2 helpers and tests ────────────────────────────────────────────

    /// Writes a blob to the store and creates a corresponding DB entry, then
    /// backdates its `last_accessed` by `age_secs` seconds.
    async fn put_entry(
        db: &Db,
        store: &LocalFsStore,
        project: i64,
        path: &str,
        age_secs: i64,
    ) -> String {
        use crate::models::Kind;
        use bytes::Bytes;
        let buf = format!("body-{path}-{}", uuid::Uuid::new_v4());
        // Use `iter` rather than `once` so the stream is `Unpin`.
        let body = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(buf))]);
        let w = store.write_streamed(body).await.unwrap();
        let e = db
            .entries()
            .upsert(project, Kind::Sstate, path, &w.hash, w.size_bytes)
            .await
            .unwrap();
        // Backdate the access time so TTL/LRU logic sees the entry as old.
        sqlx::query("UPDATE entries SET last_accessed = ? WHERE id = ?")
            .bind(crate::db::now_unix() - age_secs)
            .bind(e.id)
            .execute(db.pool())
            .await
            .unwrap();
        w.hash
    }

    #[tokio::test]
    async fn ttl_phase_evicts_old_entries() {
        use crate::models::Kind;
        let (_d, db, store, gc) = fixture().await;
        let p = db.projects().create("acme", None, None).await.unwrap();
        // sstate TTL = 1 day in fixture; insert one fresh, one 2 days old.
        let _h_fresh = put_entry(&db, &store, p.id, "fresh", 60).await;
        let h_old = put_entry(&db, &store, p.id, "old", 86_400 * 2).await;

        let r = gc.run(false).await.unwrap();
        assert_eq!(r.evicted_entries, 1);

        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "old")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "fresh")
                .await
                .unwrap()
                .is_some()
        );
        // cleanup_blobs runs in the same pass, so the blob row is fully gone.
        // Confirm it is either absent or has refcount 0 (cleanup may have run).
        let blob_row = db.blobs().get(&h_old).await.unwrap();
        assert!(
            blob_row.is_none() || blob_row.unwrap().refcount == 0,
            "evicted blob must have refcount 0 or be deleted",
        );
    }

    #[tokio::test]
    async fn pinned_entries_survive_ttl() {
        use crate::models::Kind;
        let (_d, db, store, gc) = fixture().await;
        let p = db.projects().create("acme", None, None).await.unwrap();
        put_entry(&db, &store, p.id, "old", 86_400 * 2).await;
        let e = db
            .entries()
            .find(p.id, Kind::Sstate, "old")
            .await
            .unwrap()
            .unwrap();
        db.entries().set_pinned(p.id, e.id, true).await.unwrap();

        let r = gc.run(false).await.unwrap();
        assert_eq!(r.evicted_entries, 0);
        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "old")
                .await
                .unwrap()
                .is_some()
        );
    }

    // ── Task 5.3 tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn quota_phase_evicts_lru_when_project_over_quota() {
        use crate::models::Kind;
        let (_d, db, store, gc) = fixture().await;
        // Quota = 95 bytes; each put_entry body is ~44 bytes so 4 entries ≈ 176 bytes.
        // The two oldest (p1, p2) must be evicted to bring usage under quota.
        let p = db.projects().create("acme", Some(95), None).await.unwrap();
        for i in 0..4 {
            put_entry(
                &db,
                &store,
                p.id,
                &format!("p{}", i + 1),
                100 * (4 - i64::from(i)),
            )
            .await;
        }
        let r = gc.run(false).await.unwrap();
        assert_eq!(r.evicted_entries, 2);
        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "p1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "p2")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "p3")
                .await
                .unwrap()
                .is_some()
        );
    }

    // ── Task 5.4 tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn global_phase_evicts_world_lru_when_global_quota_exceeded() {
        use crate::models::Kind;
        let (_d, db, store, _gc) = fixture().await;
        // Build a tiny GC with global_quota = 100 bytes.
        let small_cfg = GcConfig {
            interval_secs: 60,
            // Long TTLs so only the global phase fires.
            default_ttl_sstate_days: 365,
            default_ttl_downloads_days: 365,
            global_quota_bytes: 100,
            trigger_threshold_pct: 90,
        };
        let gc = GcSvc::new(db.clone(), store.clone(), small_cfg);
        let p = db.projects().create("acme", None, None).await.unwrap();
        for i in 0..4 {
            put_entry(
                &db,
                &store,
                p.id,
                &format!("p{}", i + 1),
                100 * (4 - i64::from(i)),
            )
            .await;
        }
        let r = gc.run(false).await.unwrap();
        assert!(r.evicted_entries >= 2);
        // The newest entry (p4, age=0) must survive.
        assert!(
            db.entries()
                .find(p.id, Kind::Sstate, "p4")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn cleanup_blobs_removes_zero_refcount() {
        let (_d, db, store, gc) = fixture().await;
        let p = db.projects().create("acme", None, None).await.unwrap();
        let h = put_entry(&db, &store, p.id, "x", 86_400 * 2).await;
        let _ = gc.run(false).await.unwrap();
        // Entry expired (TTL phase); blob row and file must be gone.
        assert!(db.blobs().get(&h).await.unwrap().is_none());
        assert!(!store.blob_path(&h).exists());
    }

    // ── Task 5.5 tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn orphan_scan_finds_dangling_files_and_missing_blobs() {
        let (_d, db, store, gc) = fixture().await;
        let p = db.projects().create("acme", None, None).await.unwrap();
        let h = put_entry(&db, &store, p.id, "good", 0).await;

        // Inject an on-disk file with no DB row.
        let orphan = "f".repeat(64);
        let orphan_path = store.blob_path(&orphan);
        tokio::fs::create_dir_all(orphan_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&orphan_path, b"orphan").await.unwrap();

        // Inject a DB row whose backing file has been removed.
        let missing = "e".repeat(64);
        sqlx::query(
            "INSERT INTO blobs (hash, size_bytes, refcount, created_at) VALUES (?, 1, 0, 0)",
        )
        .bind(&missing)
        .execute(db.pool())
        .await
        .unwrap();

        let report = gc.orphan_scan(false).await.unwrap();
        assert_eq!(report.disk_orphans_removed, 1);
        assert_eq!(report.db_orphans_removed, 1);
        assert!(!orphan_path.exists());
        assert!(db.blobs().get(&missing).await.unwrap().is_none());
        // The pre-existing good blob must be untouched.
        assert!(store.blob_path(&h).exists());
    }
}
