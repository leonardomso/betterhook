//! Structured output events and the TTY multiplexer.
//!
//! Every subprocess line becomes one `OutputEvent::Line` that gets shipped
//! through an mpsc channel to a single writer task. The writer holds the
//! only handle to stdout/stderr, so interleaved output from parallel jobs
//! stays line-atomic — never a garbled half-line. Phase 12 adds an NDJSON
//! sink that drains the same event stream.

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
#[derive(Debug, Clone, Copy, Default)]
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
        write_event(&ev);
    }
}

async fn json_writer(mut rx: mpsc::Receiver<OutputEvent>) {
    while let Some(ev) = rx.recv().await {
        match serde_json::to_string(&ev) {
            Ok(line) => println!("{line}"),
            Err(e) => eprintln!("betterhook: json serialize error: {e}"),
        }
    }
}

fn write_event(ev: &OutputEvent) {
    match ev {
        OutputEvent::JobStarted { job, cmd } => {
            let c = color_for(job);
            eprintln!("{} {} {}", "▶".color(c), job.color(c).bold(), cmd.dimmed());
        }
        OutputEvent::Line { job, stream, line } => {
            let c = color_for(job);
            let prefix = format!("[{job}]");
            match stream {
                Stream::Stdout => {
                    println!("{} {}", prefix.color(c), line);
                }
                Stream::Stderr => {
                    eprintln!("{} {}", prefix.color(c), line);
                }
            }
        }
        OutputEvent::JobFinished {
            job,
            exit,
            duration,
        } => {
            let c = color_for(job);
            let marker = if *exit == 0 {
                "✓".green().to_string()
            } else {
                "✗".red().to_string()
            };
            eprintln!(
                "{} {} ({}ms, exit {})",
                marker,
                job.color(c).bold(),
                duration.as_millis(),
                exit
            );
        }
        OutputEvent::JobSkipped { job, reason } => {
            let c = color_for(job);
            eprintln!(
                "{} {} {}",
                "∘".dimmed(),
                job.color(c).bold(),
                format!("skipped ({reason})").dimmed()
            );
        }
        OutputEvent::Summary {
            ok,
            jobs_run,
            jobs_skipped,
            total,
        } => {
            let marker = if *ok {
                "OK".green().bold().to_string()
            } else {
                "FAIL".red().bold().to_string()
            };
            eprintln!(
                "── {} — {jobs_run} run, {jobs_skipped} skipped, {}ms",
                marker,
                total.as_millis()
            );
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
