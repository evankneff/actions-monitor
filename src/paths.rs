//! Filesystem locations used by the app: `%APPDATA%\actions-monitor` on Windows,
//! `~/Library/Application Support/actions-monitor` on macOS.
//!
//! Each platform's rule is resolved natively from its own environment variable rather
//! than through the `dirs`/`directories` crates - see `aiDocs/architecture.md`'s
//! "Deliberately not used" table, which already rejected `dirs` for the Windows case on
//! exactly this basis ("`%APPDATA%` via `std::env::var_os` is sufficient"). The macOS
//! rule is one more `var_os` call and a fixed suffix, not enough to justify a dependency
//! that table already decided against.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// `%APPDATA%\actions-monitor`, created on demand.
#[cfg(windows)]
pub fn data_dir() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .context("the APPDATA environment variable is not set; cannot locate the config directory")?;
    let dir = PathBuf::from(appdata).join("actions-monitor");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

/// `~/Library/Application Support/actions-monitor`, created on demand - the standard
/// per-user data location on macOS, and the Windows `%APPDATA%` rule's direct
/// counterpart (SpideySense's `dirs::config_dir()` resolves to the same path; this just
/// builds it from `$HOME` instead of pulling in that crate for one join).
#[cfg(target_os = "macos")]
pub fn data_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .context("the HOME environment variable is not set; cannot locate the config directory")?;
    let dir = PathBuf::from(home).join("Library/Application Support/actions-monitor");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.toml"))
}

pub fn history_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("history.json"))
}

pub fn log_dir() -> Result<PathBuf> {
    let dir = data_dir()?.join("logs");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}
