use std::path::PathBuf;

use betterhook::cache::{Store, cache_dir};
use betterhook::git::git_common_dir;
use miette::{IntoDiagnostic, miette};

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Subcommand,
    /// Worktree root. Defaults to the current directory.
    #[arg(long, global = true)]
    pub worktree: Option<PathBuf>,
}

#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// Print entry count, total bytes, and oldest/newest mtimes as JSON.
    Stats,
    /// Remove every cached entry. Always reports the number removed.
    Clear,
    /// Walk the cache and report any corrupt entries.
    Verify,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let worktree = args.worktree.clone().unwrap_or_else(|| PathBuf::from("."));
    let common_dir = git_common_dir(&worktree)
        .await
        .map_err(|e| miette!("{e}"))?;
    let store = Store::new(&common_dir);

    match args.command {
        Subcommand::Stats => {
            let stats = store.stats().map_err(|e| miette!("{e}"))?;
            let payload = serde_json::json!({
                "cache_dir": cache_dir(&common_dir).display().to_string(),
                "entries": stats.entries,
                "total_bytes": stats.total_bytes,
                "oldest_unix": stats.oldest.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                }),
                "newest_unix": stats.newest.and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
                }),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).into_diagnostic()?
            );
        }
        Subcommand::Clear => {
            let removed = store.clear().map_err(|e| miette!("{e}"))?;
            let payload = serde_json::json!({
                "cache_dir": cache_dir(&common_dir).display().to_string(),
                "removed": removed,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).into_diagnostic()?
            );
        }
        Subcommand::Verify => {
            let corrupt = store.verify().map_err(|e| miette!("{e}"))?;
            let payload = serde_json::json!({
                "cache_dir": cache_dir(&common_dir).display().to_string(),
                "corrupt_entries": corrupt
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
                "ok": corrupt.is_empty(),
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).into_diagnostic()?
            );
            if !corrupt.is_empty() {
                std::process::exit(crate::exit_codes::GIT_ERROR);
            }
        }
    }
    Ok(())
}
