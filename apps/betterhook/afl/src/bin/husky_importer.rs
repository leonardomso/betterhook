//! afl.rs harness for the husky shell-script importer.
//!
//! The importer parses arbitrary shell scripts and turns each
//! recognized invocation into a betterhook job. Goal: find a script
//! shape that crashes the strip_runner / job_name_from logic.

use std::path::PathBuf;

use betterhook::config::import::husky;

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let Ok(s) = std::str::from_utf8(data) else {
            return;
        };
        let _ = husky::from_script(s, &PathBuf::from(".husky/pre-commit"));
    });
}
