//! Content-addressed local filesystem blob store.
//!
//! # Layout
//!
//! All data lives under a single `storage_root` directory:
//!
//! ```text
//! <root>/blobs/<aa>/<bb>/<sha256>   immutable, written via atomic rename(2) from tmp
//! <root>/tmp/<uuid>                 in-flight uploads, purged on every [`LocalFsStore::open`]
//! ```
//!
//! Blobs are sharded by the first four hex characters of their `SHA-256` digest
//! (`<aa>/<bb>`) to stay within per-directory inode limits on common filesystems.
//!
//! # Concurrency
//!
//! `LocalFsStore` is `Clone` + `Send` + `Sync`; multiple tasks may call
//! [`LocalFsStore::write_streamed`] concurrently. Each upload gets its own UUID
//! temporary file; the atomic `rename(2)` syscall makes the final placement
//! safe under concurrent writers for the same blob.
//

use std::path::{Path, PathBuf};

use futures::Stream;
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};

/// Result of a successful [`LocalFsStore::write_streamed`] call.
#[derive(Debug, Clone)]
pub struct Written {
    /// Hex-encoded `SHA-256` digest of the stored content.
    pub hash: String,
    /// Number of bytes written.
    pub size_bytes: i64,
}

/// Local filesystem blob store, content-addressed by `SHA-256`.
#[derive(Debug, Clone)]
pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    /// Opens (or initializes) a store rooted at `root`.
    ///
    /// Creates `blobs/` and `tmp/` subdirectories when missing, then removes
    /// any leftover files in `tmp/` from a previous unclean shutdown. Cleanup
    /// failures are logged as warnings and do not abort startup.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Io`] if directory creation or the `tmp/` directory
    /// listing fails.
    pub async fn open(root: &Path) -> CoreResult<Self> {
        fs::create_dir_all(root.join("blobs")).await?;
        fs::create_dir_all(root.join("tmp")).await?;

        let mut rd = fs::read_dir(root.join("tmp")).await?;
        while let Some(ent) = rd.next_entry().await? {
            // Best-effort cleanup; don't abort startup over a stuck file.
            if let Err(e) = fs::remove_file(ent.path()).await {
                tracing::warn!(
                    name: "storage.tmp.purge.failed",
                    error = %e,
                    path = ?ent.path(),
                    "tmp purge failed: {{path}}",
                );
            }
        }

        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Streams bytes to a temporary file, computes `SHA-256`, then atomically
    /// renames it to the sharded blob path.
    ///
    /// If a blob with the same digest already exists it is reused and the
    /// temporary file is discarded — writes are therefore idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Io`] on any filesystem failure, including the final
    /// atomic rename.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "chunk.len() fits in i64 for any sane upload size"
    )]
    pub async fn write_streamed<S, E>(&self, mut body: S) -> CoreResult<Written>
    where
        S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
        E: Into<std::io::Error>,
    {
        use futures::StreamExt;

        let tmp = self.root.join("tmp").join(Uuid::new_v4().to_string());
        let mut file = fs::File::create(&tmp).await?;
        let mut hasher = Sha256::new();
        let mut total: i64 = 0;

        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(Into::into)?;
            hasher.update(&chunk);
            total += chunk.len() as i64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        let hash = hex::encode(hasher.finalize());
        let dest = self.blob_path(&hash);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).await?;
        }

        if dest.exists() {
            // Existing blob wins; discard the temp file.
            let _ = fs::remove_file(&tmp).await;
        } else {
            // rename(2) is atomic when src and dst are on the same filesystem,
            // which is always the case here since both live under storage_root.
            fs::rename(&tmp, &dest).await.map_err(|e| {
                CoreError::Io(std::io::Error::new(
                    e.kind(),
                    format!("rename to {}: {e}", dest.display()),
                ))
            })?;
        }

        Ok(Written {
            hash,
            size_bytes: total,
        })
    }

    /// Returns the absolute path for a blob identified by its hex-encoded hash.
    ///
    /// The path is sharded as `<root>/blobs/<aa>/<bb>/<hex_hash>` using the
    /// first four characters of `hex_hash` to limit per-directory inode counts.
    #[must_use]
    pub fn blob_path(&self, hex_hash: &str) -> PathBuf {
        // Shard by first two hex byte pairs — 256 * 256 = 65 536 possible
        // shard directories, which keeps per-dir entry counts manageable even
        // for very large stores.
        let (a, b) = (&hex_hash[0..2], &hex_hash[2..4]);
        self.root.join("blobs").join(a).join(b).join(hex_hash)
    }

    /// Opens a blob file for streaming reads.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::NotFound`] when no blob with `hash` exists, or
    /// [`CoreError::Io`] for any other filesystem error.
    pub async fn read_blob(&self, hash: &str) -> CoreResult<fs::File> {
        let path = self.blob_path(hash);
        fs::File::open(&path).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => CoreError::NotFound {
                what: format!("blob {hash}"),
            },
            _ => CoreError::Io(e),
        })
    }

    /// Removes a blob from disk.
    ///
    /// This operation is idempotent — if the blob does not exist the call
    /// succeeds silently.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Io`] for filesystem errors other than
    /// "not found".
    pub async fn delete_blob(&self, hash: &str) -> CoreResult<()> {
        let path = self.blob_path(hash);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::Io(e)),
        }
    }

    /// Returns the storage root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures::stream;

    use super::*;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn s(chunks: Vec<&'static [u8]>) -> impl futures::Stream<Item = std::io::Result<Bytes>> {
        stream::iter(chunks.into_iter().map(|c| Ok(Bytes::copy_from_slice(c))))
    }

    #[tokio::test]
    async fn init_creates_layout() {
        let d = tmpdir();
        let store = LocalFsStore::open(d.path()).await.unwrap();
        assert!(d.path().join("blobs").is_dir());
        assert!(d.path().join("tmp").is_dir());
        let _ = store;
    }

    #[tokio::test]
    async fn open_purges_existing_tmp_files() {
        let d = tmpdir();
        std::fs::create_dir_all(d.path().join("tmp")).unwrap();
        std::fs::write(d.path().join("tmp/leftover"), b"junk").unwrap();
        let _ = LocalFsStore::open(d.path()).await.unwrap();
        assert!(!d.path().join("tmp/leftover").exists());
    }

    #[tokio::test]
    async fn write_streamed_returns_hash_and_writes_file() {
        let d = tmpdir();
        let store = LocalFsStore::open(d.path()).await.unwrap();
        let written = store
            .write_streamed(s(vec![b"hello ", b"world"]))
            .await
            .unwrap();
        // SHA-256 of "hello world"
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(written.hash, expected);
        assert_eq!(written.size_bytes, 11);
        assert!(store.blob_path(&written.hash).is_file());
    }

    #[tokio::test]
    async fn write_streamed_dedups_existing_blob() {
        let d = tmpdir();
        let store = LocalFsStore::open(d.path()).await.unwrap();
        let a = store.write_streamed(s(vec![b"abc"])).await.unwrap();
        let b = store.write_streamed(s(vec![b"abc"])).await.unwrap();
        assert_eq!(a.hash, b.hash);
        // tmp must be empty after dedup
        let mut rd = tokio::fs::read_dir(d.path().join("tmp")).await.unwrap();
        assert!(rd.next_entry().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_streamed_returns_full_body() {
        use tokio::io::AsyncReadExt;

        let d = tmpdir();
        let store = LocalFsStore::open(d.path()).await.unwrap();
        let w = store.write_streamed(s(vec![b"hello"])).await.unwrap();
        let mut buf = Vec::new();
        let mut r = store.read_blob(&w.hash).await.unwrap();
        r.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
    }

    #[tokio::test]
    async fn read_missing_blob_is_not_found() {
        let d = tmpdir();
        let store = LocalFsStore::open(d.path()).await.unwrap();
        let e = store.read_blob(&"a".repeat(64)).await.unwrap_err();
        assert!(matches!(e, crate::error::CoreError::NotFound { .. }));
    }

    #[tokio::test]
    async fn delete_blob_removes_file() {
        let d = tmpdir();
        let store = LocalFsStore::open(d.path()).await.unwrap();
        let w = store.write_streamed(s(vec![b"x"])).await.unwrap();
        store.delete_blob(&w.hash).await.unwrap();
        assert!(!store.blob_path(&w.hash).exists());
    }
}
