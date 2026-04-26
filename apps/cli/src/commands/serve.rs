use std::path::PathBuf;

use miette::IntoDiagnostic;

/// `betterhook serve` is the hidden subcommand the lock client uses to
/// spawn a coordinator daemon on demand. Keeping the daemon behind the
/// main binary means installs and wrapper scripts only need to reference
/// one executable path.
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
