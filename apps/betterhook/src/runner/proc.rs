//! Line-streaming subprocess wrapper.
//!
//! `run_command` spawns the given shell command under `sh -c`, hooks
//! stdout and stderr into the multiplexer as they emit lines, and
//! returns the exit code when the process finishes. Lines are never
//! buffered — each `readln` immediately ships an `OutputEvent::Line`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::RunError;
use super::output::{OutputEvent, Stream};

/// Run `cmd` via `sh -c`, streaming its output through `tx`.
/// Returns the exit code (`-1` on signal, whatever `status.code()` yields).
pub async fn run_command(
    job_name: &str,
    cmd: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    extra_env: &[(String, String)],
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

    let status = child.wait().await.map_err(|source| RunError::Wait {
        cmd: cmd.to_owned(),
        pid,
        source,
    })?;

    // Drain the line readers — they should already be done once the
    // process exits and closes its pipes.
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    let exit = status.code().unwrap_or(-1);
    let _ = tx
        .send(OutputEvent::JobFinished {
            job: job_name.to_owned(),
            exit,
            duration: start.elapsed(),
        })
        .await;
    Ok(exit)
}
