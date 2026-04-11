//! `betterhook import` — convert another hook manager's config into a
//! betterhook.toml. Replaces the v0 `betterhook migrate` command and
//! adds husky / hk / pre-commit alongside lefthook.

use std::path::PathBuf;

use betterhook::config::import::{self, ImportSource, MigrationReport};
use miette::{IntoDiagnostic, miette};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Path to the source config file. If omitted, betterhook
    /// auto-detects the most likely candidate in the current directory.
    #[arg(long)]
    pub from: Option<PathBuf>,
    /// Source format. If omitted, betterhook tries to infer one from
    /// the filename of `--from`.
    #[arg(long = "from-format", value_parser = ["lefthook", "husky", "hk", "pre-commit"])]
    pub source: Option<String>,
    /// Where to write the converted betterhook.toml.
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
    let from = pick_input(args)?;
    let source = pick_source(args, &from)?;

    let (raw, report) = import::import_file(source, &from)?;
    let toml_text = toml::to_string_pretty(&raw).into_diagnostic()?;
    std::fs::write(&args.to, toml_text).into_diagnostic()?;

    let notes_text = render_notes(&report, source, &from, &args.to);
    std::fs::write(&args.notes, notes_text).into_diagnostic()?;

    println!("wrote {}", args.to.display());
    println!("wrote {}", args.notes.display());
    Ok(())
}

fn pick_input(args: &Args) -> miette::Result<PathBuf> {
    if let Some(path) = &args.from {
        return Ok(path.clone());
    }
    let candidates = [
        "lefthook.yml",
        "lefthook.yaml",
        ".pre-commit-config.yaml",
        ".pre-commit-config.yml",
        "hk.toml",
        ".husky/pre-commit",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(miette!(
        "could not find a source config; pass `--from <path>`"
    ))
}

fn pick_source(args: &Args, from: &std::path::Path) -> miette::Result<ImportSource> {
    if let Some(s) = &args.source {
        return ImportSource::from_cli(s).ok_or_else(|| miette!("unknown --from-format: {s}"));
    }
    ImportSource::auto_detect(from).ok_or_else(|| {
        miette!(
            "could not auto-detect source format from {}; pass `--from-format`",
            from.display()
        )
    })
}

fn render_notes(
    report: &MigrationReport,
    source: ImportSource,
    from: &std::path::Path,
    to: &std::path::Path,
) -> String {
    use std::fmt::Write;
    let label = match source {
        ImportSource::Lefthook => "lefthook",
        ImportSource::Husky => "husky",
        ImportSource::Hk => "hk",
        ImportSource::PreCommit => "pre-commit",
    };
    let mut out = String::new();
    out.push_str("# Betterhook migration notes\n\n");
    let _ = writeln!(
        out,
        "Converted `{}` ({label}) to `{}`.\n",
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
    out.push_str("- Capability fields: add `reads = [...]`, `writes = [...]`, and `concurrent_safe = true` to enable the DAG scheduler and the CA cache.\n");
    out.push_str("- `timeout`: betterhook enforces per-job timeouts; consider setting one.\n");
    out.push_str("- `isolate`: when multiple worktrees run the same tool, declare `isolate` so the coordinator daemon can serialize.\n");
    out
}
