use std::path::PathBuf;

use betterhook::config::{self, Config};
use betterhook::dispatch::find_config;
use betterhook::runner::{RunOptions, SinkKind, run_hook_with_options};
use miette::miette;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Hook name to run (e.g. `pre-commit`).
    pub hook: String,
    /// Resolve the plan but don't execute anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Emit NDJSON events instead of TTY output.
    #[arg(long)]
    pub json: bool,
    /// Comma-separated skip list.
    #[arg(long, value_delimiter = ',')]
    pub skip: Vec<String>,
    /// Comma-separated allowlist.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Worktree root.
    #[arg(long)]
    pub worktree: Option<PathBuf>,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let worktree = args.worktree.clone().unwrap_or_else(|| PathBuf::from("."));
    let Some(path) = find_config(&worktree) else {
        return Err(miette!(
            "no betterhook config found in {}",
            worktree.display()
        ));
    };
    let cfg: Config = config::load(&path)?;
    let Some(hook) = cfg.hooks.get(&args.hook) else {
        return Err(miette!("hook '{}' is not defined", args.hook));
    };

    if args.dry_run {
        let payload = serde_json::json!({
            "config_path": path.display().to_string(),
            "hook": hook.name,
            "jobs_planned": hook.jobs.iter().map(|j| &j.name).collect::<Vec<_>>(),
        });
        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
        println!("{pretty}");
        return Ok(());
    }

    let mut options = RunOptions::from_env();
    if !args.skip.is_empty() {
        options.skip = args.skip;
    }
    if !args.only.is_empty() {
        options.only = args.only;
    }
    if args.json {
        options.sink = SinkKind::Json;
    }
    let report = run_hook_with_options(hook, &worktree, options).await?;
    if !report.ok {
        std::process::exit(crate::exit_codes::HOOK_FAILED);
    }
    Ok(())
}
