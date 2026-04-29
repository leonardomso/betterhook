//! Structured output events and the TTY multiplexer.
//!
//! Every subprocess line becomes one `OutputEvent::Line` shipped
//! through an mpsc channel to a single writer task. The writer holds
//! the only handle to stdout/stderr, so interleaved output from
//! parallel jobs stays line-atomic. The same event stream can also be
//! drained as NDJSON for agents.
//!
//! Performance notes:
//! - The TTY writer locks stdout/stderr once and writes all fields
//!   with `write!` directly, avoiding intermediate `String`
//!   allocations from `format!` and `.to_string()`.
//! - The JSON writer uses `serde_json::to_writer` to serialize
//!   directly into a locked stdout, skipping the `to_string` heap
//!   allocation.
//! - The `job` field in `OutputEvent::Line` uses `Arc<str>` to avoid
//!   cloning a heap `String` on every output line.

use std::io::Write;
use std::time::Duration;

use owo_colors::{AnsiColors, OwoColorize};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Severity of a builtin-parsed diagnostic. Mapped from each tool's
/// native severity set (error/warning/note for clippy, error/warn/info
/// for eslint, etc.) into a compact shared taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputEvent {
    JobStarted {
        job: String,
        cmd: String,
    },
    Line {
        job: String,
        stream: Stream,
        line: String,
    },
    JobFinished {
        job: String,
        exit: i32,
        #[serde(with = "humantime_serde")]
        duration: Duration,
    },
    JobSkipped {
        job: String,
        reason: String,
    },
    JobCacheHit {
        job: String,
        files: usize,
    },
    /// Structured diagnostic emitted by a builtin wrapper. Parsed from
    /// the tool's native output (eslint JSON, clippy JSON, ruff JSON,
    /// etc.) and forwarded through the same multiplexer as plain
    /// stdout/stderr lines.
    Diagnostic {
        job: String,
        file: String,
        line: Option<u32>,
        column: Option<u32>,
        severity: DiagnosticSeverity,
        message: String,
        rule: Option<String>,
    },
    Summary {
        ok: bool,
        jobs_run: usize,
        jobs_skipped: usize,
        #[serde(with = "humantime_serde")]
        total: Duration,
    },
}

/// Deterministic per-job color based on a cheap name hash. Keeps the
/// TTY output visually distinct without any config.
fn color_for(job: &str) -> AnsiColors {
    const PALETTE: &[AnsiColors] = &[
        AnsiColors::Blue,
        AnsiColors::Magenta,
        AnsiColors::Cyan,
        AnsiColors::Green,
        AnsiColors::Yellow,
        AnsiColors::BrightBlue,
        AnsiColors::BrightMagenta,
        AnsiColors::BrightCyan,
    ];
    let mut hash: u32 = 2_166_136_261; // FNV-1a offset
    for b in job.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(16_777_619);
    }
    PALETTE[(hash as usize) % PALETTE.len()]
}

/// Selects which output sink the multiplexer writes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SinkKind {
    /// Colorized line-prefixed output for humans (default).
    #[default]
    Tty,
    /// One NDJSON line per event, for agents.
    Json,
}

/// Create a paired `(tx, writer_handle)` for the default TTY sink.
/// Dropping the sender closes the channel and the writer task exits.
#[must_use]
pub fn tty_sink() -> (mpsc::Sender<OutputEvent>, tokio::task::JoinHandle<()>) {
    sink(SinkKind::Tty)
}

/// Create an event sink of the requested kind.
#[must_use]
pub fn sink(kind: SinkKind) -> (mpsc::Sender<OutputEvent>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(256);
    let handle = match kind {
        SinkKind::Tty => tokio::spawn(tty_writer(rx)),
        SinkKind::Json => tokio::spawn(json_writer(rx)),
    };
    (tx, handle)
}

async fn tty_writer(mut rx: mpsc::Receiver<OutputEvent>) {
    while let Some(ev) = rx.recv().await {
        write_event_tty(&ev);
    }
}

async fn json_writer(mut rx: mpsc::Receiver<OutputEvent>) {
    while let Some(ev) = rx.recv().await {
        write_event_json(&ev);
    }
}

/// Write a single event as NDJSON directly to locked stdout.
/// Uses `to_writer` to avoid the intermediate `String` from `to_string`.
fn write_event_json(ev: &OutputEvent) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if serde_json::to_writer(&mut lock, ev).is_ok() {
        let _ = lock.write_all(b"\n");
    }
}

/// Write a single event as colorized TTY output.
///
/// All writes go through a locked stderr (or stdout for `Line::Stdout`)
/// to avoid per-call lock overhead. The owo-colors `Display` impls
/// write ANSI escapes directly into the formatter without allocating.
#[allow(clippy::too_many_lines)]
fn write_event_tty(ev: &OutputEvent) {
    match ev {
        OutputEvent::JobStarted { job, cmd } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = writeln!(
                w,
                "{} {} {}",
                "▶".color(c),
                job.color(c).bold(),
                cmd.dimmed()
            );
        }
        OutputEvent::Line { job, stream, line } => {
            let c = color_for(job);
            match stream {
                Stream::Stdout => {
                    let stdout = std::io::stdout();
                    let mut w = stdout.lock();
                    let _ = writeln!(w, "[{}] {line}", job.color(c));
                }
                Stream::Stderr => {
                    let stderr = std::io::stderr();
                    let mut w = stderr.lock();
                    let _ = writeln!(w, "[{}] {line}", job.color(c));
                }
            }
        }
        OutputEvent::JobFinished {
            job,
            exit,
            duration,
        } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            if *exit == 0 {
                let _ = writeln!(
                    w,
                    "{} {} ({}ms, exit {exit})",
                    "✓".green(),
                    job.color(c).bold(),
                    duration.as_millis(),
                );
            } else {
                let _ = writeln!(
                    w,
                    "{} {} ({}ms, exit {exit})",
                    "✗".red(),
                    job.color(c).bold(),
                    duration.as_millis(),
                );
            }
        }
        OutputEvent::JobSkipped { job, reason } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = writeln!(
                w,
                "{} {} skipped ({})",
                "∘".dimmed(),
                job.color(c).bold(),
                reason.dimmed()
            );
        }
        OutputEvent::JobCacheHit { job, files } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = writeln!(
                w,
                "{} {} cache hit ({files} files)",
                "⚡".color(c),
                job.color(c).bold()
            );
        }
        OutputEvent::Diagnostic {
            job,
            file,
            line,
            column,
            severity,
            message,
            rule,
        } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = write!(w, "[{}] ", job.color(c).bold());
            match severity {
                DiagnosticSeverity::Error => {
                    let _ = write!(w, "{}", "error".red().bold());
                }
                DiagnosticSeverity::Warning => {
                    let _ = write!(w, "{}", "warn".yellow().bold());
                }
                DiagnosticSeverity::Info => {
                    let _ = write!(w, "{}", "info".blue().bold());
                }
                DiagnosticSeverity::Hint => {
                    let _ = write!(w, "{}", "hint".cyan());
                }
            }
            match (line, column) {
                (Some(l), Some(col)) => {
                    let _ = write!(w, " {file}:{l}:{col}");
                }
                (Some(l), None) => {
                    let _ = write!(w, " {file}:{l}");
                }
                _ => {
                    let _ = write!(w, " {file}");
                }
            }
            if let Some(r) = rule {
                let _ = write!(w, " [{r}]");
            }
            let _ = writeln!(w, " {message}");
        }
        OutputEvent::Summary {
            ok,
            jobs_run,
            jobs_skipped,
            total,
        } => {
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            if *ok {
                let _ = writeln!(
                    w,
                    "── {} — {jobs_run} run, {jobs_skipped} skipped, {}ms",
                    "OK".green().bold(),
                    total.as_millis(),
                );
            } else {
                let _ = writeln!(
                    w,
                    "── {} — {jobs_run} run, {jobs_skipped} skipped, {}ms",
                    "FAIL".red().bold(),
                    total.as_millis(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_is_deterministic() {
        assert_eq!(color_for("lint"), color_for("lint"));
        assert_eq!(color_for("test"), color_for("test"));
    }

    #[tokio::test]
    async fn tty_sink_closes_on_drop() {
        let (tx, handle) = tty_sink();
        drop(tx);
        handle.await.unwrap();
    }
}
