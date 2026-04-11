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
    /// Internal: invoked by the installed wrapper script. Not for direct use.
    #[command(name = "__dispatch", hide = true)]
    Dispatch(commands::dispatch::Args),
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => commands::init::run(&args),
        Command::Install(args) => commands::install::run(args).await,
        Command::Uninstall(args) => commands::uninstall::run(args).await,
        Command::Status(args) => commands::status::run(args).await,
        Command::Dispatch(args) => commands::dispatch::run(args).await,
    }
}
