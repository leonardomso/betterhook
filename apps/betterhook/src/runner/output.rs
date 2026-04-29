//! Structured output events and the TTY multiplexer.
//!
//! Every subprocess line becomes one `OutputEvent::Line` shipped
//! through an mpsc channel to a single writer task. The writer holds
//! the only handle to stdout/stderr, so interleaved output from
//! parallel jobs stays line-atomic. The same event stream can also be
//! drained as NDJSON for agents.
//!
//! Performance: the TTY writer locks stdout/stderr once per event and
//! writes all fields with `write!` directly. The JSON writer uses
//! `serde_json::to_writer` into locked stdout. Zero intermediate
//! `String` allocations on the hot path.

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
    HookStarted {
        hook: String,
        jobs: usize,
        parallel: bool,
    },
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

/// Format a duration for human display: "312ms", "1.2s", "2m 30s".
fn fmt_duration(d: &Duration) -> String {
    let ms = d.as_millis();
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        let secs = d.as_secs();
        format!("{}m {}s", secs / 60, secs % 60)
    }
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
fn write_event_json(ev: &OutputEvent) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if serde_json::to_writer(&mut lock, ev).is_ok() {
        let _ = lock.write_all(b"\n");
    }
}

/// Write a single event as colorized TTY output.
///
/// Design:
/// - Hook header gives context ("pre-commit | 3 jobs, parallel")
/// - Job start shows name bold, command dimmed
/// - Output lines are prefixed with dimmed job tag
/// - Success: green checkmark, duration only
/// - Failure: red X, duration + exit code (exit code is noise on success)
/// - Summary: clear pass/fail with counts and timing
#[allow(clippy::too_many_lines)]
fn write_event_tty(ev: &OutputEvent) {
    match ev {
        OutputEvent::HookStarted {
            hook,
            jobs,
            parallel,
        } => {
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let mode = if *parallel { "parallel" } else { "sequential" };
            let _ = writeln!(w);
            let _ = writeln!(
                w,
                "  {} {} {}",
                hook.bold(),
                "|".dimmed(),
                format_args!("{jobs} {}, {mode}", if *jobs == 1 { "job" } else { "jobs" }).dimmed(),
            );
            let _ = writeln!(w);
        }
        OutputEvent::JobStarted { job, cmd } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = writeln!(w, "  {} {}", "▶".color(c), job.color(c).bold());
            // Show the command on a separate indented line, dimmed.
            let _ = writeln!(w, "    {}", cmd.dimmed());
        }
        OutputEvent::Line { job, stream, line } => {
            let c = color_for(job);
            match stream {
                Stream::Stdout => {
                    let stdout = std::io::stdout();
                    let mut w = stdout.lock();
                    let _ = writeln!(
                        w,
                        "    {} {line}",
                        format_args!("{}", job.dimmed().color(c))
                    );
                }
                Stream::Stderr => {
                    let stderr = std::io::stderr();
                    let mut w = stderr.lock();
                    let _ = writeln!(
                        w,
                        "    {} {line}",
                        format_args!("{}", job.dimmed().color(c))
                    );
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
                    "  {} {} {}",
                    "✓".green(),
                    job.color(c).bold(),
                    fmt_duration(duration).dimmed(),
                );
            } else {
                let _ = writeln!(
                    w,
                    "  {} {} {} {}",
                    "✗".red(),
                    job.color(c).bold(),
                    fmt_duration(duration).dimmed(),
                    format_args!("exit {exit}").red(),
                );
            }
        }
        OutputEvent::JobSkipped { job, reason } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = writeln!(
                w,
                "  {} {} {}",
                "○".dimmed(),
                job.color(c).dimmed(),
                reason.dimmed(),
            );
        }
        OutputEvent::JobCacheHit { job, files } => {
            let c = color_for(job);
            let stderr = std::io::stderr();
            let mut w = stderr.lock();
            let _ = writeln!(
                w,
                "  {} {} {}",
                "⚡".color(c),
                job.color(c).bold(),
                format_args!("cached ({files} files)").dimmed(),
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
            let _ = write!(w, "    ");
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
            let _ = write!(w, " {}", job.color(c));
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
                let _ = write!(w, " {}", format_args!("[{r}]").dimmed());
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
            let _ = writeln!(w);
            if *ok {
                let _ = write!(w, "  {}", "PASS".green().bold());
            } else {
                let _ = write!(w, "  {}", "FAIL".red().bold());
            }
            let _ = write!(w, "  {jobs_run} run");
            if *jobs_skipped > 0 {
                let _ = write!(w, ", {jobs_skipped} skipped");
            }
            let _ = writeln!(w, "  {}", fmt_duration(total).dimmed());
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

    #[test]
    fn fmt_duration_ranges() {
        assert_eq!(fmt_duration(&Duration::from_millis(42)), "42ms");
        assert_eq!(fmt_duration(&Duration::from_millis(1500)), "1.5s");
        assert_eq!(fmt_duration(&Duration::from_secs(90)), "1m 30s");
    }

    #[tokio::test]
    async fn tty_sink_closes_on_drop() {
        let (tx, handle) = tty_sink();
        drop(tx);
        handle.await.unwrap();
    }
}
