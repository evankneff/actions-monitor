//! Coming back after something tears the window down.
//!
//! Resuming from sleep can invalidate the OpenGL surface underneath us. eframe's
//! glow backend does not treat that as recoverable - it calls
//! `make_current(..).unwrap()` - so the process dies on the resume, hours before
//! anyone looks at the screen. The window can also simply end its event loop on
//! the same event, which from the outside is indistinguishable from the tray's
//! Quit.
//!
//! Neither is fixable from here: both live inside eframe. What we can do is
//! notice that nobody asked for the window to go away and start a fresh process,
//! which gets a fresh GL context on the display that now actually exists.
//!
//! The counter guards the obvious failure of that idea. A machine that cannot
//! create a GL context at all would otherwise respawn forever, so a process that
//! dies young counts as a strike and the chain stops after a few of them.

use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};

/// Carries the strike count into the replacement process. An environment
/// variable rather than an argument so it stays out of `--help` and out of the
/// command line the user sees in Task Manager.
const RESPAWN_ENV: &str = "ACTIONS_MONITOR_RESPAWN";

/// How many short-lived restarts in a row before we accept that restarting is
/// not working.
const MAX_STRIKES: u32 = 3;

/// Live at least this long and the last restart is considered to have worked,
/// so the count goes back to zero. A resume from sleep is days apart; a GL
/// context that cannot be created fails in under a second.
const HEALTHY_UPTIME: Duration = Duration::from_secs(60);

/// Windows takes a moment to bring displays back after a resume, and a new
/// process that asks for a GL context too early just fails the same way.
const SETTLE: Duration = Duration::from_secs(3);

/// What was done about a window that went away on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    /// A replacement process is on its way; this one should just exit.
    Started,
    /// Windows is ending the session, so the window was supposed to go away.
    SessionEnding,
    /// Restarting is not working, or could not be done. Nothing else to try.
    GaveUp,
}

/// Strikes recorded against the process that is running now.
fn strikes_so_far() -> u32 {
    std::env::var(RESPAWN_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// The strike count to hand to the next process, or `None` to stop trying.
fn next_strikes(previous: u32, uptime: Duration) -> Option<u32> {
    if uptime >= HEALTHY_UPTIME {
        return Some(0);
    }
    let next = previous + 1;
    (next <= MAX_STRIKES).then_some(next)
}

/// Start a replacement process, unless doing so would be wrong or useless.
///
/// The caller should exit whatever the answer is: this process has no window
/// left to run.
pub fn respawn(uptime: Duration) -> Restart {
    // Signing out or shutting down closes the window without anyone touching
    // the tray, which from inside the event loop looks exactly like the window
    // being lost. Starting a fresh process into a session that is going away
    // would at best be pointless and at worst hold up the shutdown.
    if session_is_ending() {
        tracing::info!("the session is ending; not restarting");
        return Restart::SessionEnding;
    }

    let Some(strikes) = next_strikes(strikes_so_far(), uptime) else {
        tracing::error!(
            "the window has died {} times in a row within {}s of starting; not restarting again",
            MAX_STRIKES,
            HEALTHY_UPTIME.as_secs(),
        );
        return Restart::GaveUp;
    };

    std::thread::sleep(SETTLE);
    match spawn(strikes) {
        Ok(()) => {
            tracing::info!(strikes, "restarted actions-monitor in a fresh process");
            Restart::Started
        }
        Err(err) => {
            tracing::error!("could not restart actions-monitor: {err:#}");
            Restart::GaveUp
        }
    }
}

fn session_is_ending() -> bool {
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

fn spawn(strikes: u32) -> Result<()> {
    let exe = std::env::current_exe().context("locating our own executable")?;
    Command::new(&exe)
        .args(std::env::args().skip(1))
        .env(RESPAWN_ENV, strikes.to_string())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_lived_process_clears_the_strikes_it_inherited() {
        assert_eq!(next_strikes(2, HEALTHY_UPTIME), Some(0));
        assert_eq!(next_strikes(MAX_STRIKES, Duration::from_secs(3600)), Some(0));
    }

    #[test]
    fn a_short_lived_process_adds_a_strike() {
        assert_eq!(next_strikes(0, Duration::from_secs(1)), Some(1));
        assert_eq!(next_strikes(1, Duration::from_secs(1)), Some(2));
    }

    #[test]
    fn restarting_stops_once_it_is_clearly_not_working() {
        assert_eq!(next_strikes(MAX_STRIKES, Duration::from_secs(1)), None);
    }
}
