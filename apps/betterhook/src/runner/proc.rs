//! Line-streaming subprocess wrapper.
//!
//! `run_command` spawns the given shell command under `sh -c`, hooks
//! stdout and stderr into the multiplexer as they emit lines, and
//! returns the exit code when the process finishes. Lines are never
//! buffered — each `readln` immediately ships an `OutputEvent::Line`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Notify, mpsc};

use super::RunError;
use super::output::{OutputEvent, Stream};

/// GNU `timeout(1)` convention: the exit code we report when a job
/// exceeds its per-job timeout and we have to `SIGKILL` the child.
pub const EXIT_TIMEOUT: i32 = 124;
/// Exit code we report when a job is cancelled (`fail_fast` in a
/// sibling, `SIGINT`, etc.). Matches shell convention for `SIGINT` (130).
pub const EXIT_CANCELLED: i32 = 130;

/// Cancellation handle shared between the scheduler and every running
/// subprocess. When `cancel()` fires, every `run_command` currently
/// awaiting its child SIGKILLs the child and returns [`EXIT_CANCELLED`].
///
/// The flag is a latched `AtomicBool` — once set, it stays set, and
/// subsequent `cancelled().await` calls return immediately. This is
/// important because `tokio::sync::Notify::notify_waiters()` only
/// wakes tasks that are already awaiting at notification time; racing
/// tasks that haven't yet reached the await point would otherwise miss
/// the signal.
#[derive(Debug, Clone, Default)]
pub struct Cancel {
    inner: Arc<CancelInner>,
}

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Latch the flag and wake every current waiter.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// True once [`cancel`] has been called on any clone.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolve as soon as [`cancel`] has been called, even if the call
    /// happened before the waiter got here.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

// Internal sentinel values so the outer logic knows *why* select
// returned without holding a live borrow of `child` across the
// follow-up `start_kill` + `wait`.
enum Outcome {
    Finished(i32),
    Cancelled,
    TimedOut,
    WaitErr(std::io::Error),
}

/// Run `cmd` via `sh -c`, streaming its output through `tx`.
/// Returns the exit code (`-1` on signal, whatever `status.code()` yields,
/// [`EXIT_TIMEOUT`] on timeout, [`EXIT_CANCELLED`] on cancellation).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn run_command(
    job_name: &str,
    cmd: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    extra_env: &[(String, String)],
    timeout: Option<Duration>,
    cancel: Option<&Cancel>,
    tx: &mpsc::Sender<OutputEvent>,
) -> Result<i32, RunError> {
    let _ = tx
        .send(OutputEvent::JobStarted {
            job: job_name.to_owned(),
            cmd: cmd.to_owned(),
        })
        .await;

    let start = Instant::now();
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Essential for parallel fail_fast: if this task is aborted
        // (because a sibling job failed and the scheduler called
        // `set.abort_all()`), the child process gets SIGKILL on drop
        // instead of outliving its parent.
        .kill_on_drop(true);
    for (k, v) in env {
        command.env(k, v);
    }
    for (k, v) in extra_env {
        command.env(k, v);
    }

    let mut child = command.spawn().map_err(|source| RunError::Spawn {
        cmd: cmd.to_owned(),
        source,
    })?;
    let pid = child.id();

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let job_s = job_name.to_owned();
    let tx_s = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx_s
                .send(OutputEvent::Line {
                    job: job_s.clone(),
                    stream: Stream::Stdout,
                    line,
                })
                .await;
        }
    });

    let job_e = job_name.to_owned();
    let tx_e = tx.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx_e
                .send(OutputEvent::Line {
                    job: job_e.clone(),
                    stream: Stream::Stderr,
                    line,
                })
                .await;
        }
    });

    let cancel_fut = async {
        if let Some(c) = cancel {
            c.cancelled().await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    let outcome = {
        let wait_fut = child.wait();
        tokio::pin!(wait_fut);
        tokio::pin!(cancel_fut);
        match timeout {
            None => tokio::select! {
                biased;
                () = &mut cancel_fut => Outcome::Cancelled,
                res = &mut wait_fut => match res {
                    Ok(status) => Outcome::Finished(status.code().unwrap_or(-1)),
                    Err(e) => Outcome::WaitErr(e),
                },
            },
            Some(t) => tokio::select! {
                biased;
                () = &mut cancel_fut => Outcome::Cancelled,
                () = tokio::time::sleep(t) => Outcome::TimedOut,
                res = &mut wait_fut => match res {
                    Ok(status) => Outcome::Finished(status.code().unwrap_or(-1)),
                    Err(e) => Outcome::WaitErr(e),
                },
            },
        }
    };

    let (exit, aborted) = match outcome {
        Outcome::Finished(code) => (code, false),
        Outcome::Cancelled => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            (EXIT_CANCELLED, true)
        }
        Outcome::TimedOut => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            (EXIT_TIMEOUT, true)
        }
        Outcome::WaitErr(source) => {
            return Err(RunError::Wait {
                cmd: cmd.to_owned(),
                pid,
                source,
            });
        }
    };

    // On a clean exit, drain the line readers — they'll return
    // naturally when the child's pipes close. On cancel/timeout, the
    // child may have spawned descendants (think `sh -c 'sleep 5'`)
    // that are now orphans still holding the pipe fds, so an
    // unbounded await would hang until they die. Abort the reader
    // tasks explicitly instead.
    if aborted {
        stdout_task.abort();
        stderr_task.abort();
    }
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let _ = tx
        .send(OutputEvent::JobFinished {
            job: job_name.to_owned(),
            exit,
            duration: start.elapsed(),
        })
        .await;
    Ok(exit)
}
