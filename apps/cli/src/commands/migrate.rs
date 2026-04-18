//! Hidden compatibility alias — `betterhook migrate` was renamed to
//! `betterhook import --from-format lefthook` in phase 51. This thin
//! shim forwards the v0 flags to the import command so existing scripts
//! and docs that say `betterhook migrate --from lefthook.yml` keep
//! working for one release.

use std::path::PathBuf;

use betterhook::config::import::{self, ImportSource, MigrationReport};
use miette::{IntoDiagnostic, miette};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Path to a lefthook.yml (defaults to ./lefthook.yml).
    #[arg(long, default_value = "lefthook.yml")]
    pub from: PathBuf,
    /// Where to write the converted betterhook.toml (defaults to ./betterhook.toml).
    #[arg(long, default_value = "betterhook.toml")]
    pub to: PathBuf,
    /// Where to write the human-readable migration notes.
    #[arg(long, default_value = "BETTERHOOK_MIGRATION_NOTES.md")]
    pub notes: PathBuf,
    /// Overwrite existing output files.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: &Args) -> miette::Result<()> {
    if args.to.exists() && !args.force {
        return Err(miette!(
            "{} already exists — pass --force to overwrite",
            args.to.display()
        ));
    }

    let (raw, report) = import::import_file(ImportSource::Lefthook, &args.from)?;

    // Serialize as pretty TOML. `toml::to_string_pretty` is sufficient
    // for the forgiving top-level shape our schema uses.
    let toml_text = toml::to_string_pretty(&raw).into_diagnostic()?;
    std::fs::write(&args.to, toml_text).into_diagnostic()?;

    let notes_text = render_notes(&report, &args.from, &args.to);
    std::fs::write(&args.notes, notes_text).into_diagnostic()?;

    println!("wrote {}", args.to.display());
    println!("wrote {}", args.notes.display());
    Ok(())
}

fn render_notes(report: &MigrationReport, from: &std::path::Path, to: &std::path::Path) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    out.push_str("# Betterhook migration notes\n\n");
    let _ = writeln!(
        out,
        "Converted `{}` to `{}`.\n",
        from.display(),
        to.display()
    );
    if report.notes.is_empty() {
        out.push_str("No changes or unsupported features encountered.\n");
    } else {
        out.push_str("## Items that changed or were dropped\n\n");
        for note in &report.notes {
            let _ = writeln!(out, "- {note}");
        }
    }
    out.push_str("\n## Things to double-check\n\n");
    out.push_str("- Priority ordering: if you relied on lefthook's `parallel: true` plus implicit priorities, set an explicit `priority = [\"a\", \"b\"]` list at the hook level.\n");
    out.push_str("- `timeout`: lefthook does not enforce per-job timeouts. Betterhook does — consider setting one.\n");
    out.push_str("- `isolate`: if multiple worktrees run the same tool, declare `isolate` so the coordinator daemon can serialize.\n");
    out
}
