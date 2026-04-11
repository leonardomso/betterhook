use std::path::PathBuf;

use miette::{IntoDiagnostic, WrapErr, miette};

const STARTER_TOML: &str = "\
# betterhook config
# See https://github.com/leonardomso/betterhook for documentation.

[meta]
version = 1

[hooks.pre-commit]
parallel = true
priority = [\"fmt\", \"lint\"]

[hooks.pre-commit.jobs.fmt]
run = \"cargo fmt --all -- --check\"

[hooks.pre-commit.jobs.lint]
run = \"cargo clippy --workspace -- -D warnings\"
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
