//! Registering the app to start with Windows, via the per-user `Run` key.
//!
//! `HKCU` rather than `HKLM` so no elevation is needed, and so the entry
//! follows the user rather than the machine.
//!
//! No macOS equivalent yet - the analogous mechanism there is a `LaunchAgent` plist
//! under `~/Library/LaunchAgents`, which is different enough (a file to write and
//! `launchctl load`, not a registry value) that it is real work of its own rather than
//! a `cfg` swap. Parked; see `install`/`uninstall`/`status` below for the placeholder
//! that keeps the tray menu and CLI flags working in the meantime.

use std::path::PathBuf;

#[cfg(windows)]
use anyhow::Context;
use anyhow::Result;
#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE_NAME: &str = "ActionsMonitor";

#[cfg(windows)]
fn run_key(write: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let access = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    hkcu.open_subkey_with_flags(RUN_KEY, access)
        .with_context(|| format!("opening HKCU\\{RUN_KEY}"))
}

#[cfg(windows)]
fn command_line() -> Result<String> {
    let exe = std::env::current_exe().context("locating this executable")?;
    // Quoted so a path containing spaces (Program Files, or a user folder with
    // a space in it) still launches correctly.
    Ok(format!("\"{}\"", exe.display()))
}

/// Point the Run key at the current executable. Idempotent.
#[cfg(windows)]
pub fn install() -> Result<PathBuf> {
    let command = command_line()?;
    run_key(true)?
        .set_value(VALUE_NAME, &command)
        .with_context(|| format!("writing HKCU\\{RUN_KEY}\\{VALUE_NAME}"))?;
    std::env::current_exe().context("locating this executable")
}

/// Remove the Run key entry. Returns `false` if there was nothing to remove.
#[cfg(windows)]
pub fn uninstall() -> Result<bool> {
    let key = run_key(true)?;
    match key.delete_value(VALUE_NAME) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("deleting HKCU\\{RUN_KEY}\\{VALUE_NAME}")),
    }
}

/// The currently registered command line, if any.
#[cfg(windows)]
pub fn status() -> Result<Option<String>> {
    let key = run_key(false)?;
    match key.get_value::<String, _>(VALUE_NAME) {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context("reading the autostart entry"),
    }
}

/// Placeholder until a `LaunchAgent` implementation lands - see the module doc comment.
/// `--install-autostart` degrades to a clear error rather than a panic or a silent no-op.
#[cfg(not(windows))]
pub fn install() -> Result<PathBuf> {
    Err(anyhow::anyhow!(
        "autostart is not yet implemented on this platform (needs a LaunchAgent, not a registry key)"
    ))
}

#[cfg(not(windows))]
pub fn uninstall() -> Result<bool> {
    Err(anyhow::anyhow!("autostart is not yet implemented on this platform"))
}

/// `Ok(None)` rather than an error: the tray menu calls this on every refresh to
/// decide whether the autostart item should be ticked, and a placeholder that fails
/// loudly there would spam the log. "Not installed" is also the true answer today.
#[cfg(not(windows))]
pub fn status() -> Result<Option<String>> {
    Ok(None)
}
