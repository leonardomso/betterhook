//! afl.rs harness for the capability DAG resolver.
//!
//! The harness parses each input as a TOML config (because the resolver
//! expects already-lowered `Job`s, and constructing those by hand from
//! arbitrary bytes is awkward) and then runs `build_dag` against the
//! pre-commit hook's job list. A panic in either step is a bug; an
//! `Err` from `build_dag` is fine — `globset::Error` is a normal
//! outcome for a malformed glob.

use betterhook::config::parse::Format;
use betterhook::config::parse_bytes;
use betterhook::runner::dag::build_dag;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let Ok(s) = std::str::from_utf8(data) else {
            return;
        };
        let Ok(raw) = parse_bytes(s, Format::Toml, "afl.toml") else {
            return;
        };
        let Ok(cfg) = raw.lower() else {
            return;
        };
        if let Some(hook) = cfg.hooks.get("pre-commit") {
            let _ = build_dag(&hook.jobs);
        }
        for pkg in cfg.packages.values() {
            for hook in pkg.hooks.values() {
                let _ = build_dag(&hook.jobs);
            }
        }
    });
}
