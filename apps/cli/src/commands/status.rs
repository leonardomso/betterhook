use std::path::PathBuf;

use betterhook::status::collect;
use miette::IntoDiagnostic;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Path to inspect. Defaults to the current directory.
    #[arg(long)]
    pub worktree: Option<PathBuf>,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let status = collect(args.worktree.as_deref()).await?;
    let json = serde_json::to_string_pretty(&status).into_diagnostic()?;
    println!("{json}");
    Ok(())
}
