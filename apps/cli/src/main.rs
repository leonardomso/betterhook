use clap::{Parser, Subcommand};

mod commands;
mod exit_codes;

#[derive(Parser, Debug)]
#[command(
    name = "betterhook",
    version,
    about = "Worktree-native git hooks manager built for the AI agent era"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write a starter betterhook.toml in the current worktree.
    Init(commands::init::Args),
    /// Install worktree-aware hook wrappers into the shared .git/hooks dir.
    Install(commands::install::Args),
    /// Remove hook wrappers that were installed by betterhook.
    Uninstall(commands::uninstall::Args),
    /// Print a JSON status for this worktree (installed hooks, config, daemon).
    Status(commands::status::Args),
    /// Run a named hook directly. Supports --dry-run and --json.
    Run(commands::run::Args),
    /// Explain what a hook or job would run, without executing it.
    Explain(commands::explain::Args),
    /// Run every job's `fix = ...` variant (auto-formatting).
    Fix(commands::fix::Args),
    /// Convert a lefthook.yml to betterhook.toml.
    Migrate(commands::migrate::Args),
    /// Inspect, clear, or verify the content-addressable hook cache.
    Cache(commands::cache::Args),
    /// Internal: invoked by the installed wrapper script. Not for direct use.
    #[command(name = "__dispatch", hide = true)]
    Dispatch(commands::dispatch::Args),
    /// Internal: run the coordinator daemon. Spawned by the lock client.
    #[command(hide = true)]
    Serve(commands::serve::Args),
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => commands::init::run(&args),
        Command::Install(args) => commands::install::run(args).await,
        Command::Uninstall(args) => commands::uninstall::run(args).await,
        Command::Status(args) => commands::status::run(args).await,
        Command::Run(args) => commands::run::run(args).await,
        Command::Explain(args) => commands::explain::run(&args),
        Command::Fix(args) => commands::fix::run(args).await,
        Command::Migrate(args) => commands::migrate::run(&args),
        Command::Cache(args) => commands::cache::run(args).await,
        Command::Dispatch(args) => commands::dispatch::run(args).await,
        Command::Serve(args) => commands::serve::run(args).await,
    }
}
