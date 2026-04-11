//! Shared glob compilation helpers.
//!
//! `globset::GlobSetBuilder` is a three-step dance (new → add → build)
//! that multiple modules in the runner, watcher, and speculative
//! orchestrator used to implement inline. This module is the single
//! source of truth so every call site compiles patterns the same way.

use globset::{Glob, GlobSet, GlobSetBuilder};

/// Build a `GlobSet` from a pattern list. Returns `Ok(None)` when
/// `patterns` is empty (signalling "match nothing"); the callers
/// uniformly want `None` rather than an "always matches" set.
pub fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>, globset::Error> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        builder.add(Glob::new(pat)?);
    }
    Ok(Some(builder.build()?))
}

/// Build a `GlobSet` that always returns a set (never `None`). Used
/// when the empty case should yield an empty matcher rather than a
/// sentinel `None` — matches the semantics the watcher expects.
pub fn build_globset_always(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        builder.add(Glob::new(pat)?);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_patterns_returns_none() {
        assert!(build_globset(&[]).unwrap().is_none());
    }

    #[test]
    fn single_pattern_matches() {
        let set = build_globset(&["*.rs".to_owned()]).unwrap().unwrap();
        assert!(set.is_match("a.rs"));
        assert!(!set.is_match("a.ts"));
    }

    #[test]
    fn invalid_pattern_errors() {
        assert!(build_globset(&["[unterminated".to_owned()]).is_err());
    }

    #[test]
    fn always_yields_empty_set_for_empty_input() {
        let set = build_globset_always(&[]).unwrap();
        assert!(!set.is_match("a.rs"));
    }
}
