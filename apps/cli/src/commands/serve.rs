use std::path::PathBuf;

use miette::IntoDiagnostic;

/// `betterhook serve` is the hidden subcommand invoked when the lock
/// client needs to spawn a coordinator daemon on demand. Phase 21
/// consolidates the old `betterhookd` binary into this subcommand so
/// there's a single binary to install and a single path to bake into
/// the wrapper script.
///
/// Not listed in `--help` — users never call it directly.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Path to the unix socket to bind.
    #[arg(long)]
    pub socket: PathBuf,
}

pub async fn run(args: Args) -> miette::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    betterhook::daemon::serve(&args.socket)
        .await
        .into_diagnostic()
}
