//! `marmotte` CLI argument structures.
//

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Top-level `marmotte` CLI.
#[derive(Debug, Parser)]
#[command(name = "marmotte", version)]
pub struct Cli {
    /// Active subcommand.
    #[command(subcommand)]
    pub command: Cmd,
}

/// Available `marmotte` subcommands.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Run the HTTP server.
    Serve {
        /// Path to config `TOML`.
        #[arg(short, long, default_value = "/etc/marmotte/config.toml")]
        config: PathBuf,
    },

    /// Initialize the storage tree, run migrations, mint the first admin token.
    Init {
        /// Path to config `TOML`.
        #[arg(short, long, default_value = "/etc/marmotte/config.toml")]
        config: PathBuf,
        /// Optional file to write the admin token to (mode 0600). Stdout if absent.
        #[arg(long)]
        admin_token_out: Option<PathBuf>,
    },

    /// Upload a directory tree as cache entries.
    Push {
        /// Project name that owns the uploaded entries.
        #[arg(long)]
        project: String,
        /// Cache kind: `sstate` or `downloads`.
        #[arg(long, value_parser = ["sstate", "downloads"])]
        kind: String,
        /// Base `URL` of the Marmotte server (e.g. `http://localhost:9090/`).
        #[arg(long)]
        base_url: url::Url,
        /// `API` key for the project (HTTP Basic password).
        #[arg(long)]
        api_key: String,
        /// Maximum number of concurrent upload tasks.
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
        /// Print what would be uploaded without sending data.
        #[arg(long)]
        dry_run: bool,
        /// Source directory to upload recursively.
        dir: PathBuf,
    },
}

