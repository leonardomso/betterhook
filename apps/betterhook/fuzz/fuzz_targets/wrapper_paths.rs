#![no_main]

//! Fuzz the dispatch-time config discovery path against synthetic
//! worktree roots. The find_config + resolve combination should never
//! panic, regardless of how mangled the directory name is — dispatch
//! must cleanly fall through to NoConfig on anything that isn't a
//! real file.

use std::path::Path;

use betterhook::dispatch::find_config;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = find_config(Path::new(s));
});
