//! Worktree-aware wrapper script rendering and SHA hashing.

use sha2::{Digest, Sha256};

/// Current wrapper schema version. Bumped when the wrapper bytes change
/// in a way that requires re-install for upgrades to take effect.
pub const WRAPPER_VERSION: u32 = 1;

/// Render the POSIX wrapper script we install into
/// `<common-dir>/hooks/<hook_name>`. The wrapper is byte-identical
/// across hook names (hook name is derived from `basename "$0"`) so
/// concurrent installs from multiple worktrees don't race on content.
#[must_use]
pub fn render_wrapper(betterhook_bin: &str) -> String {
    format!(
        "#!/usr/bin/env sh
# betterhook wrapper v{WRAPPER_VERSION} — DO NOT EDIT
# Managed by betterhook. If a commit is triggering this from an
# unexpected place, run `betterhook status` to see which worktree
# and which config is in play.
set -e
hook_name=\"$(basename \"$0\")\"
worktree_root=\"$(git rev-parse --show-toplevel 2>/dev/null)\" || exit 0
if [ -n \"${{GIT_DIR:-}}\" ]; then
  exec \"{betterhook_bin}\" __dispatch \\
    --hook \"$hook_name\" \\
    --worktree \"$worktree_root\" \\
    --git-dir \"$GIT_DIR\" \\
    -- \"$@\"
fi
exec \"{betterhook_bin}\" __dispatch \\
  --hook \"$hook_name\" \\
  --worktree \"$worktree_root\" \\
  -- \"$@\"
"
    )
}

/// Compute `sha256:<hex>` for arbitrary bytes.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in digest.as_slice() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_contains_expected_markers() {
        let w = render_wrapper("/usr/local/bin/betterhook");
        assert!(w.starts_with("#!/usr/bin/env sh"));
        assert!(w.contains("rev-parse --show-toplevel"));
        assert!(w.contains("__dispatch"));
        assert!(w.contains("if [ -n \"${GIT_DIR:-}\" ]; then"));
        assert!(w.contains("/usr/local/bin/betterhook"));
        assert!(w.contains("$@"));
        assert!(w.contains("--worktree \"$worktree_root\""));
        assert!(w.contains("--git-dir \"$GIT_DIR\""));
        assert_eq!(w.matches("__dispatch").count(), 2);
        assert_eq!(w.matches("--git-dir \"$GIT_DIR\"").count(), 1);
    }

    #[test]
    fn wrapper_is_deterministic_for_same_binary() {
        let a = render_wrapper("/bin/bh");
        let b = render_wrapper("/bin/bh");
        assert_eq!(a, b);
    }

    #[test]
    fn sha256_hex_format() {
        assert_eq!(
            sha256_hex(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn wrapper_fallback_exec_omits_git_dir() {
        let w = render_wrapper("/bin/betterhook");
        let fallback = w
            .split("fi\n")
            .nth(1)
            .expect("wrapper should contain fallback exec");
        assert!(fallback.contains("__dispatch"));
        assert!(fallback.contains("--worktree \"$worktree_root\""));
        assert!(!fallback.contains("--git-dir"));
    }

    #[test]
    fn sha256_hex_is_prefixed_and_changes_with_input() {
        let empty = sha256_hex(b"");
        let hello = sha256_hex(b"hello");
        assert!(empty.starts_with("sha256:"));
        assert_eq!(empty.len(), 71);
        assert_eq!(hello.len(), 71);
        assert_ne!(empty, hello);
    }
}
