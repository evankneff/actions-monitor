//! Logging to `%APPDATA%\actions-monitor\logs\`, plus the console when there
//! is one.

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Keeps the background log-writing thread alive; drop it and buffered lines
/// are lost, so the caller must hold it for the life of the process.
pub struct LogGuard(#[allow(dead_code)] WorkerGuard);

/// Install the tracing subscriber.
///
/// `RUST_LOG` overrides everything if set; otherwise `verbose` picks between
/// info and debug for this crate, with dependencies kept at warn.
pub fn init(console: bool, verbose: bool) -> Result<LogGuard> {
    let dir = crate::paths::log_dir()?;
    let appender = tracing_appender::rolling::daily(&dir, "actions-monitor.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let level = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("warn,actions_monitor={level}")));

    let file_layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false);

    let console_layer = console.then(|| {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_target(false)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .context("installing the tracing subscriber")?;

    // eframe, winit and glow log through the `log` crate; forward those too so
    // window and GL problems land in the same file.
    let _ = tracing_log::LogTracer::init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        logs = %dir.display(),
        "actions-monitor starting"
    );
    Ok(LogGuard(guard))
}
