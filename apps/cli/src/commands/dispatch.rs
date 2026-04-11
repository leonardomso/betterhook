use std::path::PathBuf;

use betterhook::dispatch::{Dispatch, resolve};
use betterhook::runner::{RunOptions, run_hook_with_options};

use crate::exit_codes;

/// Internal runtime target of the wrapper script. Intentionally hidden
/// from `--help` because users should never call it by hand.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Hook name (e.g. `pre-commit`), derived from `basename "$0"` in
    /// the wrapper.
    #[arg(long)]
    pub hook: String,
    /// Current worktree root, captured via `git rev-parse --show-toplevel`
    /// by the wrapper. This is how we identify which config to load.
    #[arg(long)]
    pub worktree: PathBuf,
    /// The value of `$GIT_DIR` as set by git when it invoked the hook.
    /// Forwarded for future diagnostics; unused for now.
    #[arg(long)]
    pub git_dir: Option<PathBuf>,
    /// Comma-separated job names to skip. Overrides `BETTERHOOK_SKIP`.
    #[arg(long, value_delimiter = ',')]
    pub skip: Vec<String>,
    /// Comma-separated job names to run exclusively. Overrides `BETTERHOOK_ONLY`.
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Positional args passed through from git after the `--` separator.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub extra: Vec<String>,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let dispatch = resolve(&args.worktree, &args.hook)?;
    match dispatch {
        Dispatch::NoConfig | Dispatch::HookNotConfigured | Dispatch::NoJobs => Ok(()),
        Dispatch::Run { config, hook_name } => {
            let hook = config
                .hooks
                .get(&hook_name)
                .expect("hook name validated by resolve()");
            let mut options = RunOptions::from_env();
            if !args.skip.is_empty() {
                options.skip = args.skip;
            }
            if !args.only.is_empty() {
                options.only = args.only;
            }
            let report = run_hook_with_options(hook, &args.worktree, options).await?;
            if !report.ok {
                std::process::exit(exit_codes::HOOK_FAILED);
            }
            Ok(())
        }
    }
}
