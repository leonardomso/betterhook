//! Subprocess streaming, NDJSON output, cancellation, and event tests.

use std::collections::BTreeMap;
use std::time::Duration;

use betterhook::runner::output::{DiagnosticSeverity, OutputEvent, Stream};
use betterhook::runner::proc::{Cancel, EXIT_CANCELLED, EXIT_TIMEOUT, run_command};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn collect_events(
    cmd: &str,
    timeout: Option<Duration>,
    cancel: Option<&Cancel>,
) -> (i32, Vec<OutputEvent>) {
    let (tx, mut rx) = mpsc::channel(256);
    let exit = run_command(
        "test-job",
        cmd,
        std::path::Path::new("/tmp"),
        &BTreeMap::new(),
        &[],
        timeout,
        cancel,
        &tx,
    )
    .await
    .unwrap();
    drop(tx);
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }
    (exit, events)
}

// ---------------------------------------------------------------------------
// Cancel tests
// ---------------------------------------------------------------------------

#[test]
fn cancel_initially_not_cancelled() {
    let cancel = Cancel::new();
    assert!(!cancel.is_cancelled());
}

#[test]
fn cancel_after_cancel_is_set() {
    let cancel = Cancel::new();
    cancel.cancel();
    assert!(cancel.is_cancelled());
}

#[test]
fn cancel_clones_share_state() {
    let a = Cancel::new();
    let b = a.clone();
    a.cancel();
    assert!(b.is_cancelled());
}

#[test]
fn cancel_double_cancel_is_safe() {
    let cancel = Cancel::new();
    cancel.cancel();
    cancel.cancel();
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn cancel_future_resolves_immediately_when_already_set() {
    let cancel = Cancel::new();
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(100), cancel.cancelled())
        .await
        .expect("cancelled() should resolve immediately");
}

#[tokio::test]
async fn cancel_future_resolves_on_signal() {
    let cancel = Cancel::new();
    let c2 = cancel.clone();
    let handle = tokio::spawn(async move {
        c2.cancelled().await;
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancel.cancel();
    tokio::time::timeout(Duration::from_millis(200), handle)
        .await
        .expect("should not timeout")
        .expect("should not panic");
}

// ---------------------------------------------------------------------------
// EXIT constants
// ---------------------------------------------------------------------------

#[test]
fn exit_timeout_is_124() {
    assert_eq!(EXIT_TIMEOUT, 124);
}

#[test]
fn exit_cancelled_is_130() {
    assert_eq!(EXIT_CANCELLED, 130);
}

// ---------------------------------------------------------------------------
// OutputEvent serde round-trips
// ---------------------------------------------------------------------------

#[test]
fn output_event_job_started_round_trips() {
    let event = OutputEvent::JobStarted {
        job: "lint".to_owned(),
        cmd: "eslint a.ts".to_owned(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"kind\":\"job_started\""));
    let back: OutputEvent = serde_json::from_str(&json).unwrap();
    match back {
        OutputEvent::JobStarted { job, cmd } => {
            assert_eq!(job, "lint");
            assert_eq!(cmd, "eslint a.ts");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn output_event_line_round_trips() {
    let event = OutputEvent::Line {
        job: "lint".to_owned(),
        stream: Stream::Stdout,
        line: "all clean".to_owned(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"kind\":\"line\""));
    let back: OutputEvent = serde_json::from_str(&json).unwrap();
    match back {
        OutputEvent::Line { job, stream, line } => {
            assert_eq!(job, "lint");
            assert_eq!(stream, Stream::Stdout);
            assert_eq!(line, "all clean");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn output_event_job_finished_round_trips() {
    let event = OutputEvent::JobFinished {
        job: "lint".to_owned(),
        exit: 1,
        duration: Duration::from_millis(312),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"kind\":\"job_finished\""));
    let back: OutputEvent = serde_json::from_str(&json).unwrap();
    match back {
        OutputEvent::JobFinished { job, exit, .. } => {
            assert_eq!(job, "lint");
            assert_eq!(exit, 1);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn output_event_job_skipped_round_trips() {
    let event = OutputEvent::JobSkipped {
        job: "test".to_owned(),
        reason: "no matching files".to_owned(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: OutputEvent = serde_json::from_str(&json).unwrap();
    match back {
        OutputEvent::JobSkipped { job, reason } => {
            assert_eq!(job, "test");
            assert_eq!(reason, "no matching files");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn output_event_diagnostic_round_trips() {
    let event = OutputEvent::Diagnostic {
        job: "clippy".to_owned(),
        file: "src/main.rs".to_owned(),
        line: Some(42),
        column: Some(5),
        severity: DiagnosticSeverity::Warning,
        message: "unused variable".to_owned(),
        rule: Some("W0612".to_owned()),
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: OutputEvent = serde_json::from_str(&json).unwrap();
    match back {
        OutputEvent::Diagnostic {
            severity,
            line,
            column,
            rule,
            ..
        } => {
            assert_eq!(severity, DiagnosticSeverity::Warning);
            assert_eq!(line, Some(42));
            assert_eq!(column, Some(5));
            assert_eq!(rule.as_deref(), Some("W0612"));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn output_event_cache_hit_round_trips() {
    let event = OutputEvent::JobCacheHit {
        job: "lint".to_owned(),
        files: 7,
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: OutputEvent = serde_json::from_str(&json).unwrap();
    match back {
        OutputEvent::JobCacheHit { files, .. } => assert_eq!(files, 7),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn stream_variants_serialize_lowercase() {
    assert_eq!(
        serde_json::to_string(&Stream::Stdout).unwrap(),
        "\"stdout\""
    );
    assert_eq!(
        serde_json::to_string(&Stream::Stderr).unwrap(),
        "\"stderr\""
    );
}

#[test]
fn diagnostic_severity_all_variants_round_trip() {
    for sev in [
        DiagnosticSeverity::Error,
        DiagnosticSeverity::Warning,
        DiagnosticSeverity::Info,
        DiagnosticSeverity::Hint,
    ] {
        let s = serde_json::to_string(&sev).unwrap();
        let back: DiagnosticSeverity = serde_json::from_str(&s).unwrap();
        assert_eq!(sev, back);
    }
}

// ---------------------------------------------------------------------------
// run_command tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_command_exit_zero() {
    let (exit, events) = collect_events("true", None, None).await;
    assert_eq!(exit, 0);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::JobStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OutputEvent::JobFinished { exit: 0, .. }))
    );
}

#[tokio::test]
async fn run_command_nonzero_exit() {
    let (exit, _events) = collect_events("false", None, None).await;
    assert_eq!(exit, 1);
}

#[tokio::test]
async fn run_command_captures_stdout() {
    let (exit, events) = collect_events("echo hello", None, None).await;
    assert_eq!(exit, 0);
    let has_hello = events.iter().any(|e| {
        matches!(
            e,
            OutputEvent::Line { stream: Stream::Stdout, line, .. } if line == "hello"
        )
    });
    assert!(has_hello, "should capture stdout line");
}

#[tokio::test]
async fn run_command_captures_stderr() {
    let (exit, events) = collect_events("echo oops >&2", None, None).await;
    assert_eq!(exit, 0);
    let has_stderr = events.iter().any(|e| {
        matches!(
            e,
            OutputEvent::Line { stream: Stream::Stderr, line, .. } if line == "oops"
        )
    });
    assert!(has_stderr, "should capture stderr line");
}

#[tokio::test]
async fn run_command_multiline_output() {
    let (exit, events) = collect_events("printf 'a\\nb\\nc\\n'", None, None).await;
    assert_eq!(exit, 0);
    let line_count = events
        .iter()
        .filter(|e| matches!(e, OutputEvent::Line { .. }))
        .count();
    assert_eq!(line_count, 3);
}

#[tokio::test]
async fn run_command_with_timeout() {
    let (exit, _events) = collect_events("sleep 60", Some(Duration::from_millis(100)), None).await;
    assert_eq!(exit, EXIT_TIMEOUT);
}

#[tokio::test]
async fn run_command_with_cancellation() {
    let cancel = Cancel::new();
    let c2 = cancel.clone();

    let (tx, mut rx) = mpsc::channel(256);
    let tx2 = tx.clone();
    let handle = tokio::spawn(async move {
        run_command(
            "sleeper",
            "sleep 60",
            std::path::Path::new("/tmp"),
            &BTreeMap::new(),
            &[],
            None,
            Some(&c2),
            &tx2,
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let exit = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("should not timeout waiting for cancel")
        .expect("should not panic")
        .unwrap();
    assert_eq!(exit, EXIT_CANCELLED);
    drop(tx);
    while rx.recv().await.is_some() {}
}

#[tokio::test]
async fn run_command_with_env() {
    let (tx, mut rx) = mpsc::channel(256);
    let mut env = BTreeMap::new();
    env.insert("MY_VAR".to_owned(), "hello_world".to_owned());
    let exit = run_command(
        "env-test",
        "echo $MY_VAR",
        std::path::Path::new("/tmp"),
        &env,
        &[],
        None,
        None,
        &tx,
    )
    .await
    .unwrap();
    drop(tx);
    assert_eq!(exit, 0);
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }
    let has_var = events.iter().any(|e| {
        matches!(
            e,
            OutputEvent::Line { line, .. } if line == "hello_world"
        )
    });
    assert!(has_var, "env var should be visible in child");
}

#[tokio::test]
async fn run_command_with_extra_env() {
    let (tx, mut rx) = mpsc::channel(256);
    let extra = vec![("EXTRA_VAR".to_owned(), "bingo".to_owned())];
    let exit = run_command(
        "extra-env",
        "echo $EXTRA_VAR",
        std::path::Path::new("/tmp"),
        &BTreeMap::new(),
        &extra,
        None,
        None,
        &tx,
    )
    .await
    .unwrap();
    drop(tx);
    assert_eq!(exit, 0);
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }
    let has_extra = events.iter().any(|e| {
        matches!(
            e,
            OutputEvent::Line { line, .. } if line == "bingo"
        )
    });
    assert!(has_extra, "extra_env should be visible in child");
}

#[tokio::test]
async fn run_command_fast_exit_emits_start_and_finish() {
    let (exit, events) = collect_events("exit 0", None, None).await;
    assert_eq!(exit, 0);
    let has_start = events
        .iter()
        .any(|e| matches!(e, OutputEvent::JobStarted { .. }));
    let has_finish = events
        .iter()
        .any(|e| matches!(e, OutputEvent::JobFinished { .. }));
    assert!(has_start, "should always emit JobStarted");
    assert!(has_finish, "should always emit JobFinished");
}

#[tokio::test]
async fn run_command_job_finished_has_nonzero_duration() {
    let (_exit, events) = collect_events("echo hi", None, None).await;
    let finished = events
        .iter()
        .find(|e| matches!(e, OutputEvent::JobFinished { .. }));
    match finished {
        Some(OutputEvent::JobFinished { duration, .. }) => {
            assert!(duration.as_nanos() > 0, "duration should be non-zero");
        }
        _ => panic!("should have a JobFinished event"),
    }
}
