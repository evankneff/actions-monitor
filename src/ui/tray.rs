//! The notification-area (system tray) icon.
//!
//! The icon is always there, and its colour is the app's status at a glance:
//! grey when idle, blue while something is running, green/red/amber for the
//! last result. Left-clicking shows what is being watched; right-clicking opens
//! the menu, which is where the maintenance actions live.
//!
//! # Threading
//!
//! `tray-icon` needs to be created on the thread that owns the window message
//! loop, which for eframe is the thread that runs `App::new` and `App::logic` -
//! so this is constructed from `MonitorApp::new` and polled from `logic`.
//!
//! Its event handlers fire from inside the window procedure, i.e. potentially
//! while eframe is parked waiting for the next tick. They therefore do two
//! things: queue the command, and ask egui to repaint, so the click is acted on
//! immediately rather than whenever the next tick happened to be due.

use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Context, Result};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::theme;
use crate::model::{Conclusion, RunStatus};
use crate::state::AppState;

/// Something the user asked for via the tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowWatched,
    ReloadConfig,
    OpenConfig,
    OpenLogs,
    ToggleAutostart,
    Quit,
}

/// The app's overall condition, as shown by the icon's colour and tooltip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Error,
}

impl TrayState {
    /// Summarise what the UI is currently showing.
    pub fn of(state: &AppState) -> Self {
        // Deliberately the backend's live view, not the card list: a card times
        // out after half a minute, but a broken account has not.
        if !state.live_issues().is_empty() {
            return Self::Error;
        }
        let cards = state.cards();
        if cards.is_empty() {
            return Self::Idle;
        }
        if cards.iter().any(|c| c.view.status == RunStatus::InProgress) {
            return Self::Running;
        }
        if cards.iter().any(|c| c.view.status == RunStatus::Queued) {
            return Self::Queued;
        }
        // Everything finished: the worst outcome wins, so a single failure in a
        // batch is not hidden behind a success.
        let conclusions = cards.iter().filter_map(|c| c.view.conclusion);
        let mut worst = Self::Succeeded;
        for conclusion in conclusions {
            match conclusion {
                Conclusion::Failure | Conclusion::TimedOut => return Self::Failed,
                Conclusion::Cancelled | Conclusion::ActionRequired => worst = Self::Cancelled,
                _ => {}
            }
        }
        worst
    }

    fn colour(self) -> [u8; 3] {
        let c = match self {
            Self::Idle => theme::TRAY_IDLE,
            Self::Queued => theme::ACCENT_QUEUED,
            Self::Running => theme::ACCENT_RUNNING,
            Self::Succeeded => theme::ACCENT_SUCCESS,
            Self::Failed | Self::Error => theme::ACCENT_FAILURE,
            Self::Cancelled => theme::ACCENT_CANCELLED,
        };
        [c.r(), c.g(), c.b()]
    }
}

/// Summary line shown when hovering the icon.
pub fn tooltip(state: &AppState) -> String {
    if !state.live_issues().is_empty() {
        let names: Vec<&str> = state
            .live_issues()
            .iter()
            .map(|issue| issue.account.as_str())
            .collect();
        return format!("actions-monitor \u{2014} not polling: {}", names.join(", "));
    }

    let running = state
        .cards()
        .iter()
        .filter(|c| c.view.status.is_active())
        .count();
    match running {
        0 => "actions-monitor \u{2014} nothing running".to_owned(),
        1 => "actions-monitor \u{2014} 1 run in progress".to_owned(),
        n => format!("actions-monitor \u{2014} {n} runs in progress"),
    }
}

/// Menu item ids, matched against `MenuEvent::id`.
struct Ids {
    watched: MenuId,
    reload: MenuId,
    open_config: MenuId,
    open_logs: MenuId,
    autostart: MenuId,
    quit: MenuId,
}

pub struct Tray {
    icon: TrayIcon,
    rx: Receiver<TrayCommand>,
    autostart_item: CheckMenuItem,
    state: Option<TrayState>,
    tooltip: String,
    /// Kept alive because the tray holds only a `dyn ContextMenu` reference to it.
    _menu: Menu,
}

impl Tray {
    /// Build the icon and wire up its event handlers.
    ///
    /// `ctx` is used purely to wake the UI thread when a tray event arrives.
    pub fn new(ctx: egui::Context, autostart_on: bool) -> Result<Self> {
        let watched = MenuItem::with_id("watched", "Show watched repos", true, None);
        let reload = MenuItem::with_id("reload", "Reload config", true, None);
        let open_config = MenuItem::with_id("open-config", "Open config file\u{2026}", true, None);
        let open_logs = MenuItem::with_id("open-logs", "Open logs folder\u{2026}", true, None);
        let autostart_item =
            CheckMenuItem::with_id("autostart", "Start with Windows", true, autostart_on, None);
        let quit = MenuItem::with_id("quit", "Quit actions-monitor", true, None);

        let ids = Ids {
            watched: watched.id().clone(),
            reload: reload.id().clone(),
            open_config: open_config.id().clone(),
            open_logs: open_logs.id().clone(),
            autostart: autostart_item.id().clone(),
            quit: quit.id().clone(),
        };

        let menu = Menu::new();
        menu.append_items(&[
            &watched,
            &PredefinedMenuItem::separator(),
            &reload,
            &open_config,
            &PredefinedMenuItem::separator(),
            &open_logs,
            &autostart_item,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .context("building the tray menu")?;

        let (tx, rx) = channel();

        let icon = TrayIconBuilder::new()
            .with_id("actions-monitor")
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("actions-monitor \u{2014} nothing running")
            .with_icon(icon_for(TrayState::Idle)?)
            // Left click is ours (reload); the menu belongs to right click.
            .with_menu_on_left_click(false)
            .build()
            .context("creating the tray icon")?;

        install_handlers(ctx, tx, ids);

        Ok(Self {
            icon,
            rx,
            autostart_item,
            state: None,
            tooltip: String::new(),
            _menu: menu,
        })
    }

    /// Commands queued since the last call.
    pub fn drain(&self) -> Vec<TrayCommand> {
        self.rx.try_iter().collect()
    }

    /// Update the icon colour and tooltip, doing nothing when unchanged - both
    /// are shell round-trips and this is called every tick.
    pub fn refresh(&mut self, state: TrayState, tooltip: String) {
        if self.state != Some(state) {
            match icon_for(state) {
                Ok(icon) => {
                    if let Err(err) = self.icon.set_icon(Some(icon)) {
                        tracing::debug!("could not update the tray icon: {err}");
                    }
                    self.state = Some(state);
                }
                Err(err) => tracing::debug!("could not build a tray icon: {err}"),
            }
        }
        if self.tooltip != tooltip {
            if let Err(err) = self.icon.set_tooltip(Some(&tooltip)) {
                tracing::debug!("could not update the tray tooltip: {err}");
            }
            self.tooltip = tooltip;
        }
    }

    pub fn set_autostart_checked(&self, on: bool) {
        self.autostart_item.set_checked(on);
    }
}

/// Route tray and menu events into the command queue, waking the UI each time.
fn install_handlers(ctx: egui::Context, tx: Sender<TrayCommand>, ids: Ids) {
    let menu_ctx = ctx.clone();
    let menu_tx = tx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == ids.watched {
            TrayCommand::ShowWatched
        } else if event.id == ids.reload {
            TrayCommand::ReloadConfig
        } else if event.id == ids.open_config {
            TrayCommand::OpenConfig
        } else if event.id == ids.open_logs {
            TrayCommand::OpenLogs
        } else if event.id == ids.autostart {
            TrayCommand::ToggleAutostart
        } else if event.id == ids.quit {
            TrayCommand::Quit
        } else {
            return;
        };
        if menu_tx.send(command).is_ok() {
            menu_ctx.request_repaint();
        }
    }));

    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        // Fires once per hover, so it is cheap enough to keep, and it answers
        // the first troubleshooting question: is the icon receiving input?
        if let TrayIconEvent::Enter { .. } = event {
            tracing::debug!("tray icon hovered");
        }

        // Only a completed left click; `Down` and `Up` both arrive, and acting
        // on each would toggle the panel twice per click and leave it closed.
        let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        else {
            return;
        };
        if tx.send(TrayCommand::ShowWatched).is_ok() {
            ctx.request_repaint();
        }
    }));
}

/// Draw the tray icon: a filled disc in the status colour with a white play
/// triangle, echoing the Actions logo.
///
/// Generated rather than shipped as a `.ico` so the colour can track the app's
/// state, and so the binary stays a single self-contained file.
fn icon_for(state: TrayState) -> Result<Icon> {
    const SIZE: u32 = 32;
    let rgba = render(SIZE, state.colour());
    Icon::from_rgba(rgba, SIZE, SIZE).context("building the tray icon bitmap")
}

fn render(size: u32, colour: [u8; 3]) -> Vec<u8> {
    // 3x3 supersampling: at 16 device pixels the edges are very visible.
    const SS: u32 = 3;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    let s = size as f32;
    let centre = s / 2.0;
    let radius = s * 0.47;

    for y in 0..size {
        for x in 0..size {
            let (mut disc, mut glyph) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    if (px - centre).hypot(py - centre) <= radius {
                        disc += 1;
                        if in_play_triangle(px, py, s) {
                            glyph += 1;
                        }
                    }
                }
            }

            let samples = (SS * SS) as f32;
            let coverage = disc as f32 / samples;
            if coverage <= 0.0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }

            // Blend the white glyph over the disc, then premultiply nothing:
            // tray icons take straight alpha.
            let mix = glyph as f32 / samples;
            let blend = |channel: u8| -> u8 {
                (channel as f32 * (1.0 - mix) + 255.0 * mix).round().clamp(0.0, 255.0) as u8
            };
            rgba.extend_from_slice(&[
                blend(colour[0]),
                blend(colour[1]),
                blend(colour[2]),
                (coverage * 255.0).round() as u8,
            ]);
        }
    }
    rgba
}

/// A right-pointing triangle centred in the disc.
fn in_play_triangle(px: f32, py: f32, size: f32) -> bool {
    let left = size * 0.40;
    let right = size * 0.68;
    let top = size * 0.30;
    let bottom = size * 0.70;

    if px < left || px > right {
        return false;
    }
    // Linearly narrow the vertical extent from the flat left edge to the tip.
    let t = (px - left) / (right - left);
    let half = (bottom - top) / 2.0 * (1.0 - t);
    let mid = (top + bottom) / 2.0;
    (py - mid).abs() <= half
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_disc_on_a_transparent_background() {
        const SIZE: u32 = 32;
        let rgba = render(SIZE, [0x4c, 0x8d, 0xf6]);
        assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);

        let alpha_at = |x: u32, y: u32| rgba[((y * SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha_at(0, 0), 0, "corners are transparent");
        assert_eq!(alpha_at(SIZE - 1, 0), 0);
        assert_eq!(alpha_at(SIZE / 2, SIZE / 2), 255, "the centre is opaque");
    }

    #[test]
    fn the_glyph_is_lighter_than_the_disc_around_it() {
        const SIZE: u32 = 32;
        let colour = [0x2a, 0x60, 0xc0];
        let rgba = render(SIZE, colour);
        let red_at = |x: u32, y: u32| rgba[((y * SIZE + x) * 4) as usize];

        // Inside the triangle versus a point on the disc well clear of it.
        let glyph = red_at((SIZE as f32 * 0.45) as u32, SIZE / 2);
        let disc = red_at((SIZE as f32 * 0.25) as u32, SIZE / 2);
        assert_eq!(disc, colour[0], "the disc keeps the status colour");
        assert!(glyph > disc, "the play glyph is white-blended: {glyph} > {disc}");
    }

    #[test]
    fn the_triangle_narrows_towards_its_tip() {
        let size = 32.0;
        // A point just inside the flat left edge, off-centre vertically.
        assert!(in_play_triangle(size * 0.41, size * 0.36, size));
        // The same vertical offset near the tip is outside the triangle.
        assert!(!in_play_triangle(size * 0.66, size * 0.36, size));
        // The tip itself is on the centre line.
        assert!(in_play_triangle(size * 0.66, size * 0.50, size));
    }

    #[test]
    fn every_state_maps_to_a_distinct_enough_colour() {
        assert_ne!(TrayState::Running.colour(), TrayState::Idle.colour());
        assert_ne!(TrayState::Succeeded.colour(), TrayState::Failed.colour());
        assert_eq!(
            TrayState::Error.colour(),
            TrayState::Failed.colour(),
            "an auth problem should read as badly as a failed run"
        );
    }
}
