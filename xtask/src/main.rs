//! Repo-local tooling: benchmarks, stress harness, and the nightly
//! lefthook-compat suite. Invoked via `cargo xtask <subcommand>`.

use std::process::ExitCode;

mod bench_monorepo;
mod fuzz;
mod fuzz_smoke;
mod stress;

fn main() -> ExitCode {
    let mut iter = std::env::args().skip(1);
    let task = iter.next();
    let rest: Vec<String> = iter.collect();
    match task.as_deref() {
        Some("bench") => run_bench(),
        Some("bench-monorepo") => bench_monorepo::run(&rest),
        Some("stress") => stress::run(&rest),
        Some("fuzz-smoke") => fuzz_smoke::run(&rest),
        Some("fuzz") => fuzz::run(&rest),
        Some("compat") => {
            eprintln!("xtask compat runs the nightly lefthook-compat diff suite (TODO)");
            ExitCode::from(1)
        }
        _ => {
            eprintln!("usage: xtask <bench|bench-monorepo|stress|fuzz-smoke|fuzz|compat>");
            ExitCode::from(64)
        }
    }
}

fn run_bench() -> ExitCode {
    use std::process::Command;
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
