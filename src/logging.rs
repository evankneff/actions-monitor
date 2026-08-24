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

/// Send panics to the log file instead of a stderr that does not exist.
///
/// Release builds are `windows_subsystem = "windows"`, so the default hook
/// writes the panic message and location to a stderr nobody is attached to and
/// the process dies leaving nothing behind but whatever had already been
/// flushed. For an app that is supposed to sit in the tray for weeks, an
/// unexplained disappearance is the one failure mode that must not be silent.
///
/// Chains to the previous hook so `--console` still prints panics the usual way.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("<unnamed>").to_owned();
        let location = match info.location() {
            Some(at) => format!("{}:{}:{}", at.file(), at.line(), at.column()),
            None => "<unknown>".to_owned(),
        };
        // `force_capture`, not `capture`: nobody is around to have set
        // RUST_BACKTRACE on the machine that panicked three weeks into a run.
        let backtrace = std::backtrace::Backtrace::force_capture();

        tracing::error!(
            thread = %thread,
            location = %location,
            "panic: {}\n{backtrace}",
            payload_str(info.payload()),
        );

        previous(info);
    }));
}

/// The panic message, for the two payload types `panic!` actually produces.
fn payload_str(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "<non-string panic payload>"
    }
}

#[cfg(test)]
mod tests {
    use super::payload_str;

    #[test]
    fn a_static_panic_message_is_read_back_verbatim() {
        assert_eq!(payload_str(&"the window vanished"), "the window vanished");
    }

    #[test]
    fn a_formatted_panic_message_is_read_back_verbatim() {
        let owned = String::from("run 42 went missing");
        assert_eq!(payload_str(&owned), "run 42 went missing");
    }

    #[test]
    fn an_unexpected_payload_type_does_not_panic_inside_the_hook() {
        assert_eq!(payload_str(&7u32), "<non-string panic payload>");
    }
}
