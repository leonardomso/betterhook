//! Daemon lifecycle: idle-linger timer and per-platform persistent
//! unit files (launchd plists on macOS, systemd user units on Linux).
//!
//! The `betterhook install` command writes these unit files so the
//! coordinator daemon can survive reboots. It deliberately does not
//! auto-load them: launchd picks up plists on login, and systemd user
//! units still require an explicit enable/start step. The install
//! command prints the platform-specific follow-up command instead.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// How long the daemon keeps running past the last client disconnect
/// before exiting.
///
/// Kept long because the daemon also backs speculative execution and is
/// useful to keep warm while a repo is actively in use. Tests override
/// this via [`ServeOptions::idle_linger`](crate::daemon::server).
pub const IDLE_LINGER: Duration = Duration::from_hours(24);

/// Per-platform unit kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    /// macOS launchd plist under `~/Library/LaunchAgents/`.
    Launchd,
    /// systemd user unit under `~/.config/systemd/user/`.
    Systemd,
}

impl UnitKind {
    /// The kind appropriate for the current platform.
    ///
    /// Returns `None` on Windows or an unsupported OS — the install
    /// command degrades gracefully and skips unit installation.
    #[must_use]
    pub fn for_current_platform() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Self::Systemd)
        } else {
            None
        }
    }
}

/// Derive a stable unit identifier from the common dir path.
///
/// Used as the filename suffix so multiple repos coexist in the same
/// `LaunchAgents` / `systemd/user` directory without clashing.
#[must_use]
pub fn unit_id(common_dir: &Path) -> String {
    let canonical = std::fs::canonicalize(common_dir).unwrap_or_else(|_| common_dir.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(16);
    for byte in &digest.as_slice()[..8] {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The default platform-specific directory where unit files live.
///
/// Returns `None` when `$HOME` is unset or the platform isn't supported.
#[must_use]
pub fn default_unit_dir(kind: UnitKind) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    match kind {
        UnitKind::Launchd => Some(home.join("Library/LaunchAgents")),
        UnitKind::Systemd => {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from);
            Some(config.join("systemd/user"))
        }
    }
}

/// Filename for a given unit kind + id.
#[must_use]
pub fn unit_filename(kind: UnitKind, id: &str) -> String {
    match kind {
        UnitKind::Launchd => format!("com.betterhook.{id}.plist"),
        UnitKind::Systemd => format!("betterhook@{id}.service"),
    }
}

/// Render the unit file content.
#[must_use]
pub fn render_unit(kind: UnitKind, betterhook_bin: &Path, socket_path: &Path, id: &str) -> String {
    match kind {
        UnitKind::Launchd => render_launchd(betterhook_bin, socket_path, id),
        UnitKind::Systemd => render_systemd(betterhook_bin, socket_path, id),
    }
}

fn render_launchd(betterhook_bin: &Path, socket_path: &Path, id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.betterhook.{id}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>serve</string>
        <string>--socket</string>
        <string>{sock}</string>
    </array>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/betterhook-{id}.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/betterhook-{id}.err</string>
</dict>
</plist>
"#,
        bin = betterhook_bin.display(),
        sock = socket_path.display(),
    )
}

fn render_systemd(betterhook_bin: &Path, socket_path: &Path, id: &str) -> String {
    format!(
        "[Unit]
Description=betterhook coordinator daemon ({id})
After=default.target

[Service]
Type=simple
ExecStart={bin} serve --socket {sock}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
",
        bin = betterhook_bin.display(),
        sock = socket_path.display(),
    )
}

/// Outcome of installing a unit file. Returned by [`install_unit`]
/// and surfaced to the user via the install report so they can
/// finalize the bootstrap (launchctl load / systemctl --user enable).
#[derive(Debug, Clone)]
pub struct InstalledUnit {
    /// Which platform unit file format was written.
    pub kind: UnitKind,
    /// Absolute path of the written unit file.
    pub path: PathBuf,
    /// One-line shell command the user can run to load the unit.
    pub load_command: String,
}

/// Write a unit file for the given common dir. Returns metadata the
/// install command records in the manifest and surfaces to the user.
///
/// `unit_dir_override` lets tests redirect writes away from the real
/// `~/Library/LaunchAgents/` or `~/.config/systemd/user/`.
pub fn install_unit(
    common_dir: &Path,
    betterhook_bin: &Path,
    socket_path: &Path,
    unit_dir_override: Option<&Path>,
) -> std::io::Result<Option<InstalledUnit>> {
    let Some(kind) = UnitKind::for_current_platform() else {
        return Ok(None);
    };
    let id = unit_id(common_dir);
    let dir = match unit_dir_override {
        Some(d) => d.to_path_buf(),
        None => match default_unit_dir(kind) {
            Some(d) => d,
            None => return Ok(None),
        },
    };
    std::fs::create_dir_all(&dir)?;
    let filename = unit_filename(kind, &id);
    let path = dir.join(&filename);
    let content = render_unit(kind, betterhook_bin, socket_path, &id);
    std::fs::write(&path, content)?;
    let load_command = match kind {
        UnitKind::Launchd => format!("launchctl load {}", path.display()),
        UnitKind::Systemd => format!("systemctl --user enable --now betterhook@{id}.service"),
    };
    Ok(Some(InstalledUnit {
        kind,
        path,
        load_command,
    }))
}

/// Remove a previously-installed unit file.
///
/// Missing files are not an error — we just return `false`. Returns
/// `true` when a file was actually removed.
pub fn uninstall_unit(path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn unit_id_is_stable_for_same_path() {
        let dir = TempDir::new().unwrap();
        let a = unit_id(dir.path());
        let b = unit_id(dir.path());
        assert_eq!(a, b);
        assert_eq!(a.len(), 16, "expect 8 bytes hex = 16 chars");
    }

    #[test]
    fn unit_id_varies_with_path() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        assert_ne!(unit_id(a.path()), unit_id(b.path()));
    }

    #[test]
    fn render_launchd_contains_expected_elements() {
        let plist = render_unit(
            UnitKind::Launchd,
            Path::new("/usr/local/bin/betterhook"),
            Path::new("/tmp/bh.sock"),
            "abc123",
        );
        assert!(plist.contains("com.betterhook.abc123"));
        assert!(plist.contains("/usr/local/bin/betterhook"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<string>/tmp/bh.sock</string>"));
        assert!(plist.contains("RunAtLoad"));
    }

    #[test]
    fn render_systemd_contains_expected_elements() {
        let unit = render_unit(
            UnitKind::Systemd,
            Path::new("/usr/local/bin/betterhook"),
            Path::new("/tmp/bh.sock"),
            "abc123",
        );
        assert!(unit.contains("[Unit]"));
        assert!(unit.contains("Description=betterhook coordinator daemon (abc123)"));
        assert!(unit.contains("ExecStart=/usr/local/bin/betterhook serve --socket /tmp/bh.sock"));
        assert!(unit.contains("Restart=on-failure"));
    }

    #[test]
    fn install_unit_writes_to_override_dir_and_uninstall_cleans_up() {
        let common_dir = TempDir::new().unwrap();
        let unit_dir = TempDir::new().unwrap();
        let bin = Path::new("/usr/local/bin/betterhook");
        let sock = Path::new("/tmp/bh.sock");

        let result = install_unit(common_dir.path(), bin, sock, Some(unit_dir.path())).unwrap();
        let Some(installed) = result else {
            // Skip on unsupported platforms (e.g. Windows) — the function returns None.
            return;
        };
        assert!(installed.path.is_file());
        assert!(
            installed
                .load_command
                .contains(if cfg!(target_os = "macos") {
                    "launchctl"
                } else {
                    "systemctl"
                })
        );

        let removed = uninstall_unit(&installed.path).unwrap();
        assert!(removed);
        assert!(!installed.path.exists());

        // Idempotent uninstall: removing a missing file is not an error.
        assert!(!uninstall_unit(&installed.path).unwrap());
    }
}
