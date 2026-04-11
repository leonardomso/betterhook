use std::path::PathBuf;

use betterhook::dispatch::{Dispatch, resolve};
use betterhook::runner::run_hook;

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
            let report = run_hook(hook, &args.worktree).await?;
            if !report.ok {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
