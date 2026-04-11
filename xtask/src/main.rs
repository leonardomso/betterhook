//! Repo-local tooling: benchmarks, stress harness, and the nightly
//! lefthook-compat suite. Invoked via `cargo xtask <subcommand>`.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("bench") => run_bench(),
        Some("stress") => {
            eprintln!("xtask stress is a phase 18+ follow-up — see plan section §I");
            ExitCode::from(1)
        }
        Some("compat") => {
            eprintln!("xtask compat runs the nightly lefthook-compat diff suite (TODO)");
            ExitCode::from(1)
        }
        _ => {
            eprintln!("usage: xtask <bench|stress|compat>");
            ExitCode::from(64)
        }
    }
}

fn run_bench() -> ExitCode {
    let targets = ["config_parse", "output_multiplexer"];
    for target in targets {
        println!("=== cargo bench -p betterhook --bench {target} ===");
        let status = Command::new("cargo")
            .args(["bench", "-p", "betterhook", "--bench", target])
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("{target} exited with {s}");
                return ExitCode::from(1);
            }
            Err(e) => {
                eprintln!("failed to spawn cargo: {e}");
                return ExitCode::from(1);
            }
        }
    }
    println!("--- all benches ok ---");
    ExitCode::SUCCESS
}
