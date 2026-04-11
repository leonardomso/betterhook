//! The coordinator daemon binary.
//!
//! Usage:
//!
//!     betterhookd --socket /path/to/sock
//!
//! Intended to be spawned on demand by the runner when a hook declares
//! an `isolate` lock; not usually invoked by hand. See `betterhook
//! status` for introspection.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "betterhookd",
    version,
    about = "betterhook coordinator daemon for cross-worktree tool locks"
)]
struct Args {
    /// Path to the unix socket to bind.
    #[arg(long)]
    socket: PathBuf,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    betterhook::daemon::serve(&args.socket).await
}
