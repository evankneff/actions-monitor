//! Registering the app to start with Windows, via the per-user `Run` key.
//!
//! `HKCU` rather than `HKLM` so no elevation is needed, and so the entry
//! follows the user rather than the machine.

use std::path::PathBuf;

use anyhow::{Context, Result};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ActionsMonitor";

fn run_key(write: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let access = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    hkcu.open_subkey_with_flags(RUN_KEY, access)
        .with_context(|| format!("opening HKCU\\{RUN_KEY}"))
}

fn command_line() -> Result<String> {
    let exe = std::env::current_exe().context("locating this executable")?;
    // Quoted so a path containing spaces (Program Files, or a user folder with
    // a space in it) still launches correctly.
    Ok(format!("\"{}\"", exe.display()))
}

/// Point the Run key at the current executable. Idempotent.
pub fn install() -> Result<PathBuf> {
    let command = command_line()?;
    run_key(true)?
        .set_value(VALUE_NAME, &command)
        .with_context(|| format!("writing HKCU\\{RUN_KEY}\\{VALUE_NAME}"))?;
    std::env::current_exe().context("locating this executable")
}

/// Remove the Run key entry. Returns `false` if there was nothing to remove.
pub fn uninstall() -> Result<bool> {
    let key = run_key(true)?;
    match key.delete_value(VALUE_NAME) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("deleting HKCU\\{RUN_KEY}\\{VALUE_NAME}")),
    }
}

/// The currently registered command line, if any.
pub fn status() -> Result<Option<String>> {
    let key = run_key(false)?;
    match key.get_value::<String, _>(VALUE_NAME) {
        Ok(value) => Ok(Some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).context("reading the autostart entry"),
    }
}
