use std::path::PathBuf;

use miette::{IntoDiagnostic, WrapErr, miette};

const STARTER_TOML: &str = "\
# betterhook config — https://github.com/leonardomso/betterhook
#
# Capability fields (reads, writes, concurrent_safe) enable the DAG
# scheduler and the content-addressable cache. Jobs that declare them
# run in parallel automatically; jobs that don't are serialized safely.

[meta]
version = 1

[hooks.pre-commit]
parallel = true

# Use a builtin for one-line setup. Builtins fill in the run command,
# glob, capability fields, and fix variant automatically.
[hooks.pre-commit.jobs.fmt]
builtin = \"rustfmt\"

# Or declare everything manually for full control.
[hooks.pre-commit.jobs.lint]
run = \"cargo clippy --workspace --all-targets -- -D warnings\"
glob = [\"*.rs\"]
reads = [\"**/*.rs\", \"**/Cargo.toml\"]
writes = []
concurrent_safe = true

# Monorepo? Uncomment and add per-package hooks:
# [packages.web]
# path = \"apps/web\"
#
# [packages.web.hooks.pre-commit.jobs.lint]
# run = \"bun run --filter web lint\"
# reads = [\"apps/web/**\"]
# concurrent_safe = true
";

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Where to write the starter config. Defaults to ./betterhook.toml.
    #[arg(long, default_value = "betterhook.toml")]
    path: PathBuf,
    /// Overwrite an existing config file.
    #[arg(long)]
    force: bool,
}

pub fn run(args: &Args) -> miette::Result<()> {
    if args.path.exists() && !args.force {
        return Err(miette!(
            "{} already exists — pass --force to overwrite",
            args.path.display()
        ));
    }
    std::fs::write(&args.path, STARTER_TOML)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write {}", args.path.display()))?;
    println!("wrote {}", args.path.display());
    Ok(())
}
