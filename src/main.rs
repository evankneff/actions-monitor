//! actions-monitor - a background watcher for GitHub Actions runs.
//!
//! Normally invisible; a small always-on-top popup appears in the bottom-left
//! corner of the primary monitor whenever a workflow run is queued or running,
//! and disappears again shortly after the last one finishes.
//!
//! Release builds are Windows-subsystem binaries so no console flashes up on
//! login. `--console` (and every debug build) keeps one for troubleshooting.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod check;
mod config;
mod console;
mod demo;
mod filter;
mod github;
mod history;
mod logging;
mod model;
mod paths;
mod poller;
mod state;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tokio::sync::watch;

use history::History;
use model::Snapshot;

const HELP: &str = "\
actions-monitor - a desktop popup for running GitHub Actions workflows

USAGE:
    actions-monitor [OPTIONS]

OPTIONS:
    --demo                  Replay a scripted set of fake runs; never contacts
                            GitHub. Useful for checking window behaviour.
    --console               Keep a console window open with live log output.
    --verbose               Log at debug level (implies more detail in the log
                            file as well as the console).
    --config <PATH>         Use a config file other than the default.
    --check                 Verify the config without starting the UI: check
                            each token, list exactly which repositories
                            auto-discovery would watch, and confirm Actions can
                            be read on each one. Then exit.
    --install-autostart     Start actions-monitor when you sign in to Windows.
    --uninstall-autostart   Undo --install-autostart.
    --autostart-status      Show whether autostart is currently registered.
    -h, --help              Show this message.
    -V, --version           Show the version.

FILES:
    %APPDATA%\\actions-monitor\\config.toml    accounts, tokens and timings
    %APPDATA%\\actions-monitor\\history.json   past run durations, for estimates
    %APPDATA%\\actions-monitor\\logs\\          rolling daily log files
";

#[derive(Debug, Default)]
struct Args {
    demo: bool,
    console: bool,
    verbose: bool,
    config: Option<PathBuf>,
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    InstallAutostart,
    UninstallAutostart,
    AutostartStatus,
    Check,
}

fn parse_args(raw: impl Iterator<Item = String>) -> Result<Args> {
    let mut args = Args::default();
    let mut raw = raw.peekable();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--demo" => args.demo = true,
            "--console" => args.console = true,
            "--verbose" => args.verbose = true,
            "--config" => {
                let path = raw.next().context("--config needs a path")?;
                args.config = Some(PathBuf::from(path));
            }
            "--install-autostart" => args.command = Some(Command::InstallAutostart),
            "--uninstall-autostart" => args.command = Some(Command::UninstallAutostart),
            "--autostart-status" => args.command = Some(Command::AutostartStatus),
            "--check" => args.command = Some(Command::Check),
            "-h" | "--help" => args.command = Some(Command::Help),
            "-V" | "--version" => args.command = Some(Command::Version),
            other => anyhow::bail!("unrecognised option `{other}`; try --help"),
        }
    }
    Ok(args)
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            console::attach(true);
            eprintln!("actions-monitor: {err}");
            std::process::exit(2);
        }
    };

    if let Some(command) = args.command {
        console::attach(true);
        let code = run_command(command, &args);
        std::process::exit(code);
    }

    // --console allocates a window if this was launched from Explorer; a plain
    // launch still attaches to a parent terminal if there is one.
    let has_console = console::attach(args.console);

    if let Err(err) = run(&args, has_console) {
        tracing::error!("fatal: {err:#}");
        if has_console {
            eprintln!("actions-monitor: {err:#}");
        }
        std::process::exit(1);
    }
}

fn run_command(command: Command, args: &Args) -> i32 {
    match command {
        Command::Check => run_check(args),
        Command::Help => {
            println!("{HELP}");
            0
        }
        Command::Version => {
            println!("actions-monitor {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::InstallAutostart => match autostart::install() {
            Ok(exe) => {
                println!("Autostart installed. actions-monitor will start when you sign in.");
                println!("  {}", exe.display());
                println!("Remove it again with: actions-monitor --uninstall-autostart");
                0
            }
            Err(err) => {
                eprintln!("Could not install autostart: {err:#}");
                1
            }
        },
        Command::UninstallAutostart => match autostart::uninstall() {
            Ok(true) => {
                println!("Autostart removed.");
                0
            }
            Ok(false) => {
                println!("Autostart was not installed; nothing to do.");
                0
            }
            Err(err) => {
                eprintln!("Could not remove autostart: {err:#}");
                1
            }
        },
        Command::AutostartStatus => match autostart::status() {
            Ok(Some(command)) => {
                println!("Autostart is installed: {command}");
                0
            }
            Ok(None) => {
                println!("Autostart is not installed.");
                0
            }
            Err(err) => {
                eprintln!("Could not read the autostart entry: {err:#}");
                1
            }
        },
    }
}

/// Dry-run the whole configuration and report on it, without starting the UI.
fn run_check(args: &Args) -> i32 {
    let config_path = match args.config.clone().map(Ok).unwrap_or_else(paths::config_path) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Could not locate the config: {err:#}");
            return 1;
        }
    };
    let config = match config::load(&config_path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Could not read {}:\n  {err:#}", config_path.display());
            return 1;
        }
    };

    check::header(&config_path, &config);
    if config.accounts.is_empty() {
        println!("No [[accounts]] blocks configured, so there is nothing to check.");
        return 1;
    }

    // A current-thread runtime is plenty: this is a handful of sequential
    // requests, and keeping it single-threaded keeps the output in order.
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("Could not start the async runtime: {err}");
            return 1;
        }
    };
    if runtime.block_on(check::run(&config)) { 0 } else { 1 }
}

fn run(args: &Args, has_console: bool) -> Result<()> {
    let _log_guard = logging::init(has_console && args.console, args.verbose)?;

    let config_path = match &args.config {
        Some(path) => path.clone(),
        None => paths::config_path()?,
    };

    // In demo mode we never read the config, so a first run can be inspected
    // before any tokens exist.
    let config = if args.demo {
        config::Config::default()
    } else {
        match first_run_setup(&config_path, has_console)? {
            Some(config) => config,
            None => return Ok(()),
        }
    };

    let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(Snapshot::empty(config.linger())));
    // Demo mode gets a tray icon too: it is there to preview the whole app, and
    // the config-related menu items explain themselves when there is no config.
    let want_tray = config.show_tray_icon;

    // The config channel is created here rather than inside the backend, so the
    // UI can share the reloader and drive a reload from the tray menu.
    let (config_tx, config_rx) = watch::channel(Arc::new(config));
    let reloader = (!args.demo).then(|| Arc::new(config::Reloader::new(config_path, config_tx)));

    let demo = args.demo;
    let backend_reloader = reloader.clone();
    let native_options = eframe::NativeOptions {
        viewport: ui::viewport(),
        ..Default::default()
    };

    eframe::run_native(
        "actions-monitor",
        native_options,
        Box::new(move |cc| {
            // The backend only exists once we have an egui Context to wake.
            let ctx = cc.egui_ctx.clone();
            if let Err(err) = start_backend(ctx, snapshot_tx, config_rx, backend_reloader, demo) {
                tracing::error!("could not start the polling backend: {err:#}");
            }
            Ok(Box::new(ui::MonitorApp::new(
                cc,
                snapshot_rx,
                reloader,
                want_tray,
            )))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
    .context("running the popup window")
}

/// Load the config, creating and opening the template if this is a first run.
///
/// Returns `None` when the caller should exit quietly (template just written,
/// or no accounts configured yet).
///
/// `interactive` says whether a person is watching, i.e. whether we were run
/// from a terminal. It gates the "here is your config" editor pop-up: helpful
/// when you just typed the command, unwelcome every single time you sign in.
fn first_run_setup(config_path: &Path, interactive: bool) -> Result<Option<config::Config>> {
    if config::ensure_template(config_path)? {
        let message = format!(
            "Welcome to actions-monitor.\n\n\
             A configuration template has been created at:\n  {}\n\n\
             Fill in your GitHub account(s) and token(s), save the file, then \
             start actions-monitor again.\n\
             The template explains exactly which token permissions are needed.",
            config_path.display()
        );
        tracing::info!("wrote a fresh config template to {}", config_path.display());
        println!("{message}");
        if let Err(err) = open::that_detached(config_path) {
            tracing::warn!("could not open the config in an editor: {err}");
            println!("(Open it yourself; this machine has no editor registered for .toml files.)");
        }
        return Ok(None);
    }

    let config = config::load(config_path)
        .with_context(|| format!("loading {}", config_path.display()))?;

    if config.accounts.is_empty() {
        let message = format!(
            "actions-monitor has nothing to watch yet.\n\n\
             Add at least one [[accounts]] block to:\n  {}\n\n\
             The commented example in that file shows the shape.",
            config_path.display()
        );
        tracing::warn!("no accounts configured; exiting");
        println!("{message}");
        // Started at sign-in with an unfinished config: say so in the log and
        // get out of the way. Opening an editor uninvited on every login would
        // be worse than doing nothing.
        if interactive {
            if let Err(err) = open::that_detached(config_path) {
                tracing::warn!("could not open the config in an editor: {err}");
            }
        } else {
            tracing::warn!(
                "run actions-monitor from a terminal to have the config opened for editing"
            );
        }
        return Ok(None);
    }

    Ok(Some(config))
}

/// Spin up the tokio runtime on its own thread and start polling.
///
/// The UI thread belongs to winit, so everything async lives here and talks to
/// the UI over a watch channel plus `Context::request_repaint`.
fn start_backend(
    ctx: egui::Context,
    snapshot_tx: watch::Sender<Arc<Snapshot>>,
    config_rx: watch::Receiver<Arc<config::Config>>,
    reloader: Option<Arc<config::Reloader>>,
    demo: bool,
) -> Result<()> {
    let history_path = paths::history_path()?;
    let history = History::load(&history_path).unwrap_or_else(|err| {
        tracing::warn!("could not read run history ({err:#}); starting a fresh one");
        History::default()
    });
    let history = Arc::new(Mutex::new(history));

    std::thread::Builder::new()
        .name("backend".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("actions-monitor-io")
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::error!("could not start the async runtime: {err}");
                    return;
                }
            };

            let notify: poller::Notify = Arc::new(move || ctx.request_repaint());

            runtime.block_on(async move {
                if demo {
                    demo::run(snapshot_tx, notify).await;
                    return;
                }

                // Held for the lifetime of the runtime; dropping it stops the
                // filesystem watch and with it hot reloading.
                let _watcher = reloader.and_then(|reloader| {
                    config::spawn_watcher(reloader)
                        .inspect_err(|err| {
                            tracing::warn!(
                                "config hot-reload unavailable ({err:#}); \
                                 use the tray menu, or restart, after editing the config"
                            );
                        })
                        .ok()
                });

                poller::run(config_rx, snapshot_tx, history, history_path, notify).await;
                tracing::info!("polling backend stopped");
            });
        })
        .context("spawning the backend thread")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Result<Args> {
        parse_args(items.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn flags_parse() {
        let parsed = args(&["--demo", "--console", "--verbose"]).expect("parse");
        assert!(parsed.demo && parsed.console && parsed.verbose);
        assert!(parsed.command.is_none());
    }

    #[test]
    fn config_path_can_be_overridden() {
        let parsed = args(&["--config", "C:/tmp/x.toml"]).expect("parse");
        assert_eq!(parsed.config, Some(PathBuf::from("C:/tmp/x.toml")));
        assert!(args(&["--config"]).is_err(), "a bare --config is an error");
    }

    #[test]
    fn autostart_and_help_are_commands() {
        assert_eq!(
            args(&["--install-autostart"]).expect("parse").command,
            Some(Command::InstallAutostart)
        );
        assert_eq!(
            args(&["--uninstall-autostart"]).expect("parse").command,
            Some(Command::UninstallAutostart)
        );
        assert_eq!(args(&["-h"]).expect("parse").command, Some(Command::Help));
        assert_eq!(args(&["-V"]).expect("parse").command, Some(Command::Version));
    }

    #[test]
    fn unknown_options_are_rejected_rather_than_ignored() {
        let err = args(&["--wat"]).expect_err("should fail");
        assert!(err.to_string().contains("--wat"));
    }
}
