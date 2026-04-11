use std::path::PathBuf;

use betterhook::install::{InstallOptions, install};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Install wrappers only for these hook types (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub hook: Option<Vec<String>>,
    /// Unset an existing `core.hooksPath` owned by another hooks tool.
    #[arg(long)]
    pub takeover: bool,
    /// Explicit config file to load (defaults to ./betterhook.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Skip writing the launchd/systemd persistent unit file.
    /// The wrapper still works via on-demand daemon spawn.
    #[arg(long)]
    pub no_unit: bool,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let opts = InstallOptions {
        worktree: None,
        config_path: args.config,
        only_hooks: args.hook,
        takeover: args.takeover,
        skip_unit: args.no_unit,
        unit_dir_override: None,
    };
    let report = install(opts).await?;
    println!("betterhook installed {} wrappers:", report.installed.len());
    for name in &report.installed {
        println!("  {}", report.hooks_dir.join(name).display());
    }
    println!("manifest: {}", report.manifest_path.display());
    if let Some(unit) = &report.unit {
        println!();
        println!("wrote persistent unit: {}", unit.path.display());
        println!("run once to finish: {load}", load = unit.load_command);
    }
    Ok(())
}
