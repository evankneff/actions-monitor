//! Filesystem locations used by the app, all rooted at `%APPDATA%\actions-monitor`.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// `%APPDATA%\actions-monitor`, created on demand.
pub fn data_dir() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .context("the APPDATA environment variable is not set; cannot locate the config directory")?;
    let dir = PathBuf::from(appdata).join("actions-monitor");
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
