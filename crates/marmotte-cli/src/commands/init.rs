//! `marmotte init`: bootstrap the DB and mint an admin token.
//
// Rust guideline compliant 2026-05-06

use std::path::PathBuf;

use marmotte_core::{
    auth::{generate_secret, hash_argon2, lookup_hash},
    config::Config,
    db::Db,
    storage::LocalFsStore,
};
use tokio::io::AsyncWriteExt;

use crate::error::CliResult;

/// Bootstraps the storage tree, runs migrations, and mints a first admin token.
///
/// If `admin_token_out` is given, the token secret is written to that file with
/// mode 0600. Otherwise the secret is printed to stdout.
///
/// # Errors
///
/// Returns an error if the config file cannot be read, the database fails to
/// initialize, token creation fails, or file I/O fails.
pub async fn run(config: PathBuf, admin_token_out: Option<PathBuf>) -> CliResult<()> {
    let cfg = Config::load(&config)?;
    let _store = LocalFsStore::open(&cfg.server.storage_root).await?;
    let db = Db::connect(&cfg.database).await?;

    let secret = generate_secret();
    let lookup = lookup_hash(&secret);
    let phc = hash_argon2(&secret)?;
    let token = db
        .admin_tokens()
        .create(&lookup, &phc, Some("init"))
        .await?;

    println!("admin token id: {}", token.id);
    if let Some(out) = admin_token_out {
        let mut f = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&out)
            .await?;
        f.write_all(secret.as_bytes()).await?;
        f.write_all(b"\n").await?;
        eprintln!("wrote admin token to {}", out.display());
    } else {
        println!("{secret}");
    }

    db.close().await;
    Ok(())
}

// Rust guideline compliant 2026-05-06
