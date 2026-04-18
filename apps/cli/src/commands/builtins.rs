//! `betterhook builtins` — discovery endpoint for agents.
//!
//! * `list` prints every registered builtin with its capability defaults
//!   and the tool binary it calls.
//! * `show <name>` prints the full default job template so agents can
//!   copy-paste it into a `betterhook.toml` or construct a config
//!   programmatically.

use betterhook::builtins::{self, BuiltinMeta};
use miette::{IntoDiagnostic, miette};

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Subcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum Subcommand {
    /// Print every registered builtin as JSON.
    List,
    /// Print the default job template for one builtin by name.
    Show {
        /// Name of the builtin (e.g. `rustfmt`, `clippy`).
        name: String,
    },
}

pub fn run(args: Args) -> miette::Result<()> {
    match args.command {
        Subcommand::List => {
            let entries: Vec<_> = builtins::registry().values().map(meta_to_json).collect();
            let payload = serde_json::json!({ "builtins": entries });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).into_diagnostic()?
            );
        }
        Subcommand::Show { name } => {
            let meta = builtins::get(&name).ok_or_else(|| miette!("no such builtin: {name}"))?;
            let payload = meta_to_json(&meta);
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).into_diagnostic()?
            );
        }
    }
    Ok(())
}

fn meta_to_json(m: &BuiltinMeta) -> serde_json::Value {
    serde_json::json!({
        "name": m.id.0,
        "description": m.description,
        "run": m.run,
        "fix": m.fix,
        "glob": m.glob,
        "reads": m.reads,
        "writes": m.writes,
        "network": m.network,
        "concurrent_safe": m.concurrent_safe,
        "tool_binary": m.tool_binary,
    })
}
