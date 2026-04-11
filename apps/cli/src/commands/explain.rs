use std::path::PathBuf;

use betterhook::config;
use betterhook::dispatch::find_config;
use miette::{IntoDiagnostic, miette};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Hook name. Required if --job is scoped to a hook with siblings.
    #[arg(long)]
    pub hook: String,
    /// Job name to explain. If omitted, every job in the hook is listed.
    #[arg(long)]
    pub job: Option<String>,
    /// Path to inspect. Defaults to the current directory.
    #[arg(long)]
    pub worktree: Option<PathBuf>,
}

pub fn run(args: &Args) -> miette::Result<()> {
    let worktree = args.worktree.clone().unwrap_or_else(|| PathBuf::from("."));
    let Some(path) = find_config(&worktree) else {
        return Err(miette!(
            "no betterhook config found in {}",
            worktree.display()
        ));
    };
    let cfg = config::load(&path)?;
    let hook = cfg
        .hooks
        .get(&args.hook)
        .ok_or_else(|| miette!("hook '{}' is not defined in {}", args.hook, path.display()))?;

    let mut payload = serde_json::json!({
        "config_path": path.display().to_string(),
        "hook": {
            "name": hook.name,
            "parallel": hook.parallel,
            "fail_fast": hook.fail_fast,
            "stash_untracked": hook.stash_untracked,
        },
    });
    let mut jobs = Vec::new();
    for job in &hook.jobs {
        if let Some(filter) = args.job.as_deref() {
            if filter != job.name {
                continue;
            }
        }
        jobs.push(serde_json::json!({
            "name": job.name,
            "run": job.run,
            "fix": job.fix,
            "glob": job.glob,
            "exclude": job.exclude,
            "env": job.env,
            "timeout": job.timeout.map(|d| format!("{}ms", d.as_millis())),
            "stage_fixed": job.stage_fixed,
            "interactive": job.interactive,
            "priority": job.priority,
            "isolate": format!("{:?}", job.isolate),
        }));
    }
    payload["jobs"] = serde_json::Value::Array(jobs);

    let pretty = serde_json::to_string_pretty(&payload).into_diagnostic()?;
    println!("{pretty}");
    Ok(())
}
