use tokio::sync::mpsc;

use super::output::{OutputEvent, Stream};

/// Collect all stdout/stderr lines from the captured events, feed them
/// through the builtin's `parse_output`, and emit one `Diagnostic`
/// event per finding.
pub(super) async fn emit_builtin_diagnostics(
    builtin_id: &str,
    job_name: &str,
    captured: &[OutputEvent],
    tx: &mpsc::Sender<OutputEvent>,
) {
    let Some(meta) = crate::builtins::get(builtin_id) else {
        return;
    };
    let mut stdout = String::new();
    for ev in captured {
        if let OutputEvent::Line {
            stream: Stream::Stdout,
            line,
            ..
        } = ev
        {
            stdout.push_str(line);
            stdout.push('\n');
        }
    }
    if stdout.is_empty() {
        return;
    }
    let diags = match builtin_id {
        "rustfmt" => crate::builtins::rustfmt::parse_output(&stdout),
        "clippy" => crate::builtins::clippy::parse_output(&stdout),
        "prettier" => crate::builtins::prettier::parse_output(&stdout),
        "eslint" => crate::builtins::eslint::parse_output(&stdout),
        "ruff" => crate::builtins::ruff::parse_output(&stdout),
        "black" => crate::builtins::black::parse_output(&stdout),
        "gofmt" => crate::builtins::gofmt::parse_output(&stdout),
        "govet" => crate::builtins::govet::parse_output(&stdout),
        "biome" => crate::builtins::biome::parse_output(&stdout),
        "oxlint" => crate::builtins::oxlint::parse_output(&stdout),
        "shellcheck" => crate::builtins::shellcheck::parse_output(&stdout),
        "gitleaks" => crate::builtins::gitleaks::parse_output(&stdout),
        _ => return,
    };
    let _ = meta;
    for d in diags {
        let _ = tx
            .send(OutputEvent::Diagnostic {
                job: job_name.to_owned(),
                file: d.file,
                line: d.line,
                column: d.column,
                severity: d.severity,
                message: d.message,
                rule: d.rule,
            })
            .await;
    }
}
