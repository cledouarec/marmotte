//! Marmotte CLI entry point.
//
// Rust guideline compliant 2026-05-06

mod cli;
mod commands;
mod error;

use clap::Parser;

use crate::{
    cli::{Cli, Cmd},
    error::CliResult,
};

/// Parses CLI arguments and dispatches to the appropriate subcommand handler.
///
/// # Errors
///
/// Propagates errors from the invoked subcommand.
#[tokio::main]
async fn main() -> CliResult<()> {
    let args = Cli::parse();
    match args.command {
        Cmd::Serve { config } => commands::serve::run(config).await,
        Cmd::Init {
            config,
            admin_token_out,
        } => commands::init::run(config, admin_token_out).await,
        Cmd::Push {
            project,
            kind,
            base_url,
            api_key,
            concurrency,
            dry_run,
            dir,
        } => commands::push::run(project, kind, base_url, api_key, concurrency, dry_run, dir).await,
    }
}

// Rust guideline compliant 2026-05-06
