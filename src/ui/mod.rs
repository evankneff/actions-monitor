//! The popup window.
//!
//! The window exists for the whole life of the process but is only *shown* when
//! there is something to show.
//!
//! eframe splits the app in two: `logic` runs on every tick, including while the
//! window is hidden (no egui pass at all then, so no UI state is disturbed), and
//! `ui` runs only when there is something to paint. That maps exactly onto what
//! this app needs - `logic` notices new snapshots, sizes and anchors the window,
//! and shows or hides it; `ui` just draws the stack.
//!
//! Because sizing happens before any drawing, card heights must be known up
//! front rather than measured: see the fixed-height layout in [`theme`].

pub mod card;
pub mod theme;
pub mod tray;
pub mod win;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use egui::Context;
use tokio::sync::watch;
use windows::Win32::Foundation::HWND;

use crate::config::{Reloaded, Reloader};
use crate::model::{RunKey, Snapshot};
use crate::state::AppState;
use tray::{Tray, TrayCommand, TrayState};
use win::Placement;

/// Tick rate while the window is on screen (elapsed times, pulsing bar).
const ACTIVE_TICK: Duration = Duration::from_millis(50);
/// Tick rate while only finished cards remain: nothing is animating, we are
/// just waiting out the linger.
const LINGER_TICK: Duration = Duration::from_millis(200);
/// Safety-net tick while hidden. New snapshots wake us immediately through
/// `Context::request_repaint`; this only covers anything that slips past that.
const IDLE_TICK: Duration = Duration::from_secs(5);

/// Which element the pointer was over last frame, so backgrounds can light up
/// before this frame's response for that card exists.
#[derive(Default)]
struct HoverMap {
    runs: HashMap<RunKey, card::Hovered>,
    issues: HashMap<String, card::Hovered>,
    notices: HashMap<String, card::Hovered>,
}

enum Action {
    ClosePanel,
    Open(String),
    Dismiss(RunKey),
    DismissIssue(String),
    DismissNotice(String),
}

pub struct MonitorApp {
    snapshot_rx: watch::Receiver<Arc<Snapshot>>,
    state: AppState,
    hwnd: Option<HWND>,
    visible: bool,
    last_placement: Option<Placement>,
    hover: HoverMap,
    /// `None` in demo mode, and if the tray could not be created - the app is
    /// still perfectly usable without it.
    tray: Option<Tray>,
    /// `None` in demo mode, where there is no config to reload.
    reloader: Option<Arc<Reloader>>,
}

impl MonitorApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        snapshot_rx: watch::Receiver<Arc<Snapshot>>,
        reloader: Option<Arc<Reloader>>,
        want_tray: bool,
    ) -> Self {
        let hwnd = win::hwnd_of(cc);
        match hwnd {
            Some(hwnd) => {
                win::configure(hwnd);
                win::strip_dwm_frame(hwnd);
                tracing::info!(
                    hwnd = ?hwnd.0,
                    "popup window configured: no focus steal, no taskbar entry"
                );
            }
            None => tracing::warn!(
                "could not resolve the native window handle; \
                 falling back to default window behaviour"
            ),
        }

        cc.egui_ctx.all_styles_mut(|style| {
            // Single-line everywhere: a wrapped commit subject would make the
            // card taller than the window was sized for.
            style.wrap_mode = Some(egui::TextWrapMode::Truncate);
            style.interaction.selectable_labels = false;
        });

        // The tray icon has to be built on this thread: it needs the window
        // message loop that eframe is about to run.
        let tray = want_tray
            .then(|| {
                let autostart_on = autostart_enabled();
                Tray::new(cc.egui_ctx.clone(), autostart_on)
                    .inspect_err(|err| tracing::warn!("no tray icon: {err:#}"))
                    .ok()
            })
            .flatten();
        if tray.is_some() {
            tracing::info!("tray icon created");
        }

        let linger = snapshot_rx.borrow().linger;
        Self {
            snapshot_rx,
            state: AppState::new(linger),
            hwnd,
            visible: false,
            last_placement: None,
            hover: HoverMap::default(),
            tray,
            reloader,
        }
    }

    /// Act on anything the user asked for through the tray.
    fn handle_tray(&mut self, ctx: &Context, now: Instant) {
        let Some(tray) = &self.tray else { return };
        for command in tray.drain() {
            tracing::debug!(?command, "tray command");
            match command {
                TrayCommand::ShowWatched => self.state.toggle_panel(now),
                TrayCommand::ReloadConfig => self.reload_config(),
                TrayCommand::OpenConfig => self.open_config(),
                TrayCommand::OpenLogs => self.open_logs(),
                TrayCommand::ToggleAutostart => self.toggle_autostart(),
                TrayCommand::Quit => {
                    tracing::info!("quitting on request from the tray menu");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn reload_config(&mut self) {
        let Some(reloader) = self.reloader.clone() else {
            self.state
                .push_notice("Demo mode", "There is no config to reload", false);
            return;
        };
        match reloader.reload() {
            Ok(Reloaded::Changed { accounts }) => self.state.push_notice(
                "Config reloaded",
                match accounts {
                    1 => "Now watching 1 account".to_owned(),
                    n => format!("Now watching {n} accounts"),
                },
                false,
            ),
            Ok(Reloaded::Unchanged) => {
                self.state
                    .push_notice("Config reloaded", "No changes since last time", false);
            }
            Err(err) => {
                tracing::warn!("manual reload failed: {err:#}");
                self.state
                    .push_notice("Config not reloaded", first_line(&format!("{err}")), true);
            }
        }
    }

    fn open_config(&mut self) {
        let Some(reloader) = &self.reloader else {
            self.state
                .push_notice("Demo mode", "There is no config file in use", false);
            return;
        };
        let path = reloader.path().to_path_buf();
        if let Err(err) = open::that_detached(&path) {
            tracing::warn!("could not open the config: {err}");
            self.state
                .push_notice("Could not open the config", first_line(&err.to_string()), true);
        }
    }

    fn open_logs(&mut self) {
        let opened = crate::paths::log_dir()
            .map_err(|err| format!("{err:#}"))
            .and_then(|dir| open::that_detached(&dir).map_err(|err| err.to_string()));
        if let Err(err) = opened {
            tracing::warn!("could not open the log folder: {err}");
            self.state
                .push_notice("Could not open the logs", first_line(&err), true);
        }
    }

    /// Flip the autostart registry entry, then put the menu tick in sync with
    /// what actually happened rather than with what was clicked.
    fn toggle_autostart(&mut self) {
        let currently_on = autostart_enabled();
        let result = if currently_on {
            crate::autostart::uninstall().map(|_| false)
        } else {
            crate::autostart::install().map(|_| true)
        };

        match result {
            Ok(now_on) => {
                if let Some(tray) = &self.tray {
                    tray.set_autostart_checked(now_on);
                }
                tracing::info!(enabled = now_on, "autostart toggled from the tray");
                let (title, body) = if now_on {
                    (
                        "Starting with Windows",
                        "actions-monitor will run when you sign in",
                    )
                } else {
                    (
                        "No longer starting with Windows",
                        "Re-enable it from the tray menu at any time",
                    )
                };
                self.state.push_notice(title, body, false);
            }
            Err(err) => {
                tracing::warn!("could not change autostart: {err:#}");
                if let Some(tray) = &self.tray {
                    tray.set_autostart_checked(currently_on);
                }
                self.state.push_notice(
                    "Could not change autostart",
                    first_line(&format!("{err}")),
                    true,
                );
            }
        }
    }

    /// Total window height for the current card stack, in logical points.
    fn wanted_height(&self) -> f32 {
        let runs = self.state.cards().len();
        let messages = self.state.issues().len() + self.state.notices().len();
        let panel = usize::from(self.state.panel_open());
        let count = runs + messages + panel;
        if count == 0 {
            return 1.0;
        }
        let mut cards =
            runs as f32 * theme::RUN_CARD_HEIGHT + messages as f32 * theme::ISSUE_CARD_HEIGHT;
        if panel == 1 {
            cards += theme::panel_height(card::panel_rows(self.state.watched()));
        }
        theme::STACK_PAD * 2.0 + cards + theme::CARD_GAP * (count - 1) as f32
    }

    /// Resize and re-anchor the window to the bottom-left of the primary
    /// monitor's work area. Re-read every tick, so a resolution change, a moved
    /// taskbar or a DPI change is picked up without a restart.
    fn reposition(&mut self, ctx: &Context) {
        let Some(hwnd) = self.hwnd else { return };

        // The window's own DPI is authoritative; egui's points-per-pixel is a
        // good fallback if Windows will not answer.
        let scale = win::dpi_scale(hwnd).unwrap_or_else(|| ctx.pixels_per_point());
        let px = |points: f32| (points * scale).round() as i32;

        let placement = win::bottom_left(
            win::primary_work_area(),
            px(theme::WINDOW_WIDTH),
            px(self.wanted_height()).max(1),
            px(theme::SCREEN_MARGIN),
        );

        if self.last_placement != Some(placement) {
            win::apply_placement(hwnd, placement);
            self.last_placement = Some(placement);
        }
    }

    /// Re-assert the popup's extended styles, which winit resets whenever it
    /// rebuilds them from its own flag set.
    ///
    /// Windows only re-evaluates taskbar presence when a window is shown, so if
    /// the styles were clobbered while the popup was on screen, hide and show it
    /// again to make the correction take effect.
    fn enforce_styles(&mut self) {
        let Some(hwnd) = self.hwnd else { return };
        if !win::configure(hwnd) {
            return;
        }
        tracing::debug!("re-applied popup window styles after winit reset them");
        if self.visible {
            win::hide(hwnd);
            win::show(hwnd);
        }
    }

    fn update_visibility(&mut self) {
        let Some(hwnd) = self.hwnd else { return };
        let wanted = !self.state.is_empty();

        if wanted && !self.visible {
            win::show(hwnd);
            self.visible = true;
            tracing::debug!(cards = self.state.cards().len(), "popup shown");
        } else if !wanted && self.visible {
            win::hide(hwnd);
            self.visible = false;
            self.hover = HoverMap::default();
            tracing::debug!("popup hidden");
        }
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::Open(url) => {
                tracing::debug!(%url, "opening run in the default browser");
                if let Err(err) = open::that_detached(&url) {
                    tracing::warn!(%url, "could not open the browser: {err}");
                }
            }
            Action::Dismiss(key) => {
                tracing::debug!(run_id = key.run_id, "card dismissed");
                self.state.dismiss(&key);
            }
            Action::DismissIssue(account) => self.state.dismiss_issue(&account),
            Action::DismissNotice(title) => self.state.dismiss_notice(&title),
            Action::ClosePanel => self.state.close_panel(),
        }
    }
}

fn autostart_enabled() -> bool {
    crate::autostart::status()
        .inspect_err(|err| tracing::warn!("could not read the autostart entry: {err:#}"))
        .is_ok_and(|entry| entry.is_some())
}

/// Registry and IO errors can be long or multi-line; a card has room for one.
fn first_line(text: &str) -> String {
    crate::model::truncate_chars(text.lines().next().unwrap_or(text).trim(), 60)
}

impl eframe::App for MonitorApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Fully transparent: only the cards are painted, so the gaps between
        // them and their rounded corners show the desktop through.
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Runs whether or not the window is visible.
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();

        self.handle_tray(ctx, now);

        if self.snapshot_rx.has_changed().unwrap_or(false) {
            let snapshot = self.snapshot_rx.borrow_and_update().clone();
            self.state.apply(&snapshot, now);
        }
        self.state.retire_expired(now);

        if let Some(tray) = &mut self.tray {
            tray.refresh(TrayState::of(&self.state), tray::tooltip(&self.state));
        }

        // Size and place before showing, so the window never appears at the
        // wrong height and then jumps.
        self.enforce_styles();
        self.reposition(ctx);
        self.update_visibility();

        if self.visible {
            // A stack of finished cards has nothing left to animate except its
            // own disappearance, so it does not need the full frame rate.
            let tick = if self.state.has_live_run() {
                ACTIVE_TICK
            } else {
                LINGER_TICK
            };
            ctx.request_repaint_after(tick);
        } else {
            // Nothing to draw, so just make sure we wake up again: the backend
            // will normally beat this timer to it.
            let wake = self
                .state
                .next_expiry(now)
                .map_or(IDLE_TICK, |until| until.min(IDLE_TICK));
            ctx.request_repaint_after(wake);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let now_utc = Utc::now();
        let mut actions: Vec<Action> = Vec::new();
        let mut next_hover = HoverMap::default();

        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;

        // Inset horizontally so drop shadows are not clipped by the window edge.
        ui.horizontal(|ui| {
            ui.add_space(theme::CARD_INSET);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.set_width(theme::CARD_WIDTH);
                ui.add_space(theme::STACK_PAD);

                let mut first = true;
                if self.state.panel_open() {
                    first = false;
                    let was = self
                        .hover
                        .notices
                        .get("__panel")
                        .copied()
                        .unwrap_or_default();
                    let (hit, hovered) =
                        card::watched_panel(ui, self.state.watched(), now, was);
                    next_hover.notices.insert("__panel".to_owned(), hovered);
                    if hit == card::Hit::Dismiss {
                        actions.push(Action::ClosePanel);
                    }
                }

                for notice in self.state.notices() {
                    if !first {
                        ui.add_space(theme::CARD_GAP);
                    }
                    first = false;

                    let was = self
                        .hover
                        .notices
                        .get(&notice.title)
                        .copied()
                        .unwrap_or_default();
                    let (hit, hovered) = card::notice_card(ui, notice, was);
                    next_hover.notices.insert(notice.title.clone(), hovered);
                    if hit == card::Hit::Dismiss {
                        actions.push(Action::DismissNotice(notice.title.clone()));
                    }
                }

                for issue in self.state.issues() {
                    if !first {
                        ui.add_space(theme::CARD_GAP);
                    }
                    first = false;

                    let was = self
                        .hover
                        .issues
                        .get(&issue.account)
                        .copied()
                        .unwrap_or_default();
                    let (hit, hovered) = card::issue_card(ui, issue, was);
                    next_hover.issues.insert(issue.account.clone(), hovered);
                    if hit == card::Hit::Dismiss {
                        actions.push(Action::DismissIssue(issue.account.clone()));
                    }
                }

                for run in self.state.cards() {
                    if !first {
                        ui.add_space(theme::CARD_GAP);
                    }
                    first = false;

                    let was = self
                        .hover
                        .runs
                        .get(&run.view.key)
                        .copied()
                        .unwrap_or_default();
                    let (hit, hovered) = card::run_card(ui, run, was, now_utc, now);
                    next_hover.runs.insert(run.view.key.clone(), hovered);
                    match hit {
                        card::Hit::Open => actions.push(Action::Open(run.view.html_url.clone())),
                        card::Hit::Dismiss => actions.push(Action::Dismiss(run.view.key.clone())),
                        card::Hit::None => {}
                    }
                }
            });
        });

        self.hover = next_hover;
        for action in actions {
            self.apply_action(action);
        }
    }
}

/// The viewport configuration that makes this an unobtrusive overlay.
pub fn viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("actions-monitor")
        .with_app_id("actions-monitor")
        .with_inner_size([theme::WINDOW_WIDTH, theme::RUN_CARD_HEIGHT])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        // Created hidden and inactive; `win::configure` then makes it
        // impossible for the window to take focus even once shown.
        .with_active(false)
        .with_visible(false)
        .with_has_shadow(false)
        .with_drag_and_drop(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The height arithmetic used to size the window, isolated from egui.
    fn height_for(runs: usize, issues: usize) -> f32 {
        let count = runs + issues;
        if count == 0 {
            return 1.0;
        }
        theme::STACK_PAD * 2.0
            + runs as f32 * theme::RUN_CARD_HEIGHT
            + issues as f32 * theme::ISSUE_CARD_HEIGHT
            + theme::CARD_GAP * (count - 1) as f32
    }

    #[test]
    fn an_empty_stack_collapses_to_nothing() {
        assert_eq!(height_for(0, 0), 1.0);
    }

    #[test]
    fn each_extra_card_adds_its_height_plus_one_gap() {
        let one = height_for(1, 0);
        let two = height_for(2, 0);
        assert_eq!(one, theme::STACK_PAD * 2.0 + theme::RUN_CARD_HEIGHT);
        assert_eq!(two - one, theme::RUN_CARD_HEIGHT + theme::CARD_GAP);
    }

    #[test]
    fn issue_cards_are_measured_with_their_own_height() {
        let mixed = height_for(1, 1);
        assert_eq!(
            mixed,
            theme::STACK_PAD * 2.0
                + theme::RUN_CARD_HEIGHT
                + theme::ISSUE_CARD_HEIGHT
                + theme::CARD_GAP
        );
    }
}
