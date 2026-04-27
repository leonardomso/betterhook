use std::path::PathBuf;

use betterhook::config::{self, Config, Hook};
use betterhook::dispatch::find_config;
use betterhook::runner::{RunOptions, run_hook_with_options};
use miette::miette;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Hook whose jobs should run in fix mode. Defaults to `pre-commit`.
    #[arg(long, default_value = "pre-commit")]
    pub hook: String,
    /// Restrict to these job names (otherwise every job with a `fix` variant runs).
    #[arg(long, value_delimiter = ',')]
    pub job: Vec<String>,
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

    // Build a synthetic hook where each job's `run` is replaced with
    // its `fix` variant. Jobs without a fix are skipped.
    let fix_hook = swap_in_fix_commands(hook, &args.job);
    if fix_hook.jobs.is_empty() {
        println!("no jobs in '{}' declare a `fix` variant", args.hook);
        return Ok(());
    }

    let options = RunOptions::from_env();
    let report = run_hook_with_options(&fix_hook, &worktree, options).await?;
    if !report.ok {
        std::process::exit(crate::exit_codes::HOOK_FAILED);
    }
    Ok(())
}

fn swap_in_fix_commands(hook: &Hook, job_filter: &[String]) -> Hook {
    let mut out = hook.clone();
    out.jobs.retain(|j| {
        if !job_filter.is_empty() && !job_filter.iter().any(|n| n == j.name.as_str()) {
            return false;
        }
        j.fix.is_some()
    });
    for j in &mut out.jobs {
        if let Some(fix_cmd) = j.fix.clone() {
            j.run = fix_cmd;
        }
    }
    out
}
