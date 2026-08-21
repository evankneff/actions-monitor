//! Painting for a single card.
//!
//! Nothing inside a card is an interactive egui widget: the whole card is one
//! click target, and the close control is resolved from the click position.
//! That keeps hit-testing unambiguous no matter how egui orders widgets, and
//! lets the card behave as a single surface.
//!
//! Every row is allocated at an exact height with item spacing switched off, so
//! a card's total height is the compile-time constant in [`theme`]. The window
//! is sized from that constant before any drawing happens.

use std::time::Duration;

use chrono::{DateTime, Utc};
use egui::{Align, Color32, Label, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2, vec2};

use super::theme;
use crate::history::{self, Progress};
use crate::model::{
    AccountIssue, RunStatus, RunView, WatchedRepo, format_ago, format_duration, truncate_chars,
};
use crate::state::{Card, Notice, status_line};

/// Size of the close control, and how far outside it a click still counts.
const CLOSE_SIZE: f32 = 14.0;
const CLOSE_SLOP: f32 = 5.0;
/// Reserved width for the right-aligned elapsed time.
const ELAPSED_WIDTH: f32 = 54.0;
/// How long a newly-arrived card takes to fade in.
const APPEAR: Duration = Duration::from_millis(220);

/// What the user did to a card this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    None,
    Open,
    Dismiss,
}

/// Hover state carried over from the previous frame, so a card's background can
/// be lit before this frame's response for it exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hovered {
    pub card: bool,
    pub close: bool,
}

pub fn run_card(
    ui: &mut Ui,
    card: &Card,
    hovered: Hovered,
    now_utc: DateTime<Utc>,
    now: std::time::Instant,
) -> (Hit, Hovered) {
    let view = &card.view;
    let accent = theme::accent_for(view);
    let elapsed = view.elapsed(now_utc, now);
    let mut close_rect = Rect::NOTHING;

    // Fade a card in rather than having it pop into the stack. The window is
    // already sized for it, so only the paint is animated, never the layout.
    let appear = ease_out(
        now.saturating_duration_since(card.first_seen)
            .div_duration_f32(APPEAR)
            .min(1.0),
    );
    ui.multiply_opacity(appear);

    let rect = shell(hovered.card)
        .show(ui, |ui| {
            prepare(ui, theme::RUN_CONTENT_HEIGHT);

            close_rect = title_row(
                ui,
                RichText::new(&view.repo)
                    .size(theme::TEXT_TITLE)
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
                hovered.close,
            );
            ui.add_space(theme::GAPS[0]);

            row(
                ui,
                theme::ROW_META,
                RichText::new(format!("{}  #{}", view.workflow, view.run_number))
                    .size(theme::TEXT_BODY)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(theme::GAPS[1]);

            row(
                ui,
                theme::ROW_COMMIT,
                RichText::new(commit_line(view))
                    .size(theme::TEXT_SMALL)
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(theme::GAPS[2]);

            status_row(ui, view, accent, elapsed);
            ui.add_space(theme::GAPS[3]);

            progress_bar(ui, view, elapsed, accent);
        })
        .response
        .rect;

    paint_accent(ui, rect, accent);
    let hit = finish(ui, rect, close_rect, ui.make_persistent_id(&view.key), true);
    if appear < 1.0 {
        // Keep the animation going even if nothing else asks for a repaint.
        ui.ctx().request_repaint();
    }
    hit
}

/// Decelerating ease, so the fade lands softly.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// A card explaining that an account has stopped polling, e.g. a bad token.
pub fn issue_card(ui: &mut Ui, issue: &AccountIssue, hovered: Hovered) -> (Hit, Hovered) {
    message_card(
        ui,
        ui.make_persistent_id(("issue", &issue.account)),
        &format!("{} \u{2014} not polling", issue.account),
        &issue.message,
        "Fix the token in config.toml \u{2014} it reloads on save",
        theme::ACCENT_FAILURE,
        hovered,
    )
}

/// A card for something the UI itself wants to say, such as the result of a
/// tray-triggered config reload.
pub fn notice_card(ui: &mut Ui, notice: &Notice, hovered: Hovered) -> (Hit, Hovered) {
    let accent = if notice.is_error {
        theme::ACCENT_FAILURE
    } else {
        theme::ACCENT_RUNNING
    };
    message_card(
        ui,
        ui.make_persistent_id(("notice", &notice.title)),
        &notice.title,
        &notice.body,
        "",
        accent,
        hovered,
    )
}

/// Three lines of text with an accent stripe: the shared shape behind account
/// issues and UI notices. Clicking anywhere dismisses it, since unlike a run
/// card there is nowhere useful to navigate to.
fn message_card(
    ui: &mut Ui,
    id: egui::Id,
    title: &str,
    body: &str,
    hint: &str,
    accent: Color32,
    hovered: Hovered,
) -> (Hit, Hovered) {
    let mut close_rect = Rect::NOTHING;

    let rect = shell(hovered.card)
        .show(ui, |ui| {
            prepare(ui, theme::ISSUE_CONTENT_HEIGHT);

            close_rect = title_row(
                ui,
                RichText::new(title)
                    .size(theme::TEXT_TITLE)
                    .color(accent)
                    .strong(),
                hovered.close,
            );
            ui.add_space(theme::GAPS[0]);

            row(
                ui,
                theme::ROW_META,
                RichText::new(body)
                    .size(theme::TEXT_BODY)
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(theme::GAPS[1]);

            row(
                ui,
                theme::ROW_COMMIT,
                RichText::new(hint)
                    .size(theme::TEXT_SMALL)
                    .color(theme::TEXT_MUTED),
            );
        })
        .response
        .rect;

    paint_accent(ui, rect, accent);
    finish(ui, rect, close_rect, id, false)
}

/// The watched-repos panel: every repository being polled, grouped by account,
/// with how long ago each was last checked.
///
/// Answers "is it actually looking at the thing I care about?", which is
/// otherwise invisible when auto-discovery decides the list.
pub fn watched_panel(
    ui: &mut Ui,
    watched: &[WatchedRepo],
    now: std::time::Instant,
    hovered: Hovered,
) -> (Hit, Hovered) {
    let rows = panel_rows(watched);
    let mut close_rect = Rect::NOTHING;

    let rect = shell(hovered.card)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(theme::CONTENT_WIDTH);

            let heading = match watched.len() {
                0 => "Watching nothing yet".to_owned(),
                1 => "Watching 1 repository".to_owned(),
                n => format!("Watching {n} repositories"),
            };
            close_rect = title_row(
                ui,
                RichText::new(heading)
                    .size(theme::TEXT_TITLE)
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
                hovered.close,
            );
            ui.add_space(theme::PANEL_HEAD_GAP);

            if watched.is_empty() {
                panel_row(
                    ui,
                    RichText::new("the first poll has not finished yet")
                        .size(theme::TEXT_SMALL)
                        .color(theme::TEXT_MUTED),
                    None,
                );
                return;
            }

            let mut account: Option<&str> = None;
            let shown = watched.len().min(theme::PANEL_MAX_ROWS);
            for repo in &watched[..shown] {
                // A heading each time the account changes, so several tokens
                // are legible without repeating the account on every line.
                if account != Some(repo.account.as_str()) {
                    account = Some(&repo.account);
                    panel_row(
                        ui,
                        RichText::new(&repo.account)
                            .size(theme::TEXT_SMALL)
                            .color(theme::ACCENT_RUNNING),
                        None,
                    );
                }

                let (colour, right) = if repo.readable {
                    let when = repo
                        .last_checked
                        .map(|at| format_ago(now.saturating_duration_since(at)));
                    (theme::TEXT_SECONDARY, when.unwrap_or_else(|| "pending".to_owned()))
                } else {
                    (theme::ACCENT_FAILURE, "unreadable".to_owned())
                };
                panel_row(
                    ui,
                    RichText::new(format!("  {}", truncate_chars(&repo.repo, 34)))
                        .size(theme::TEXT_SMALL)
                        .color(colour),
                    Some(right),
                );
            }

            if watched.len() > shown {
                panel_row(
                    ui,
                    RichText::new(format!("  \u{2026} and {} more", watched.len() - shown))
                        .size(theme::TEXT_SMALL)
                        .color(theme::TEXT_MUTED),
                    None,
                );
            }
        })
        .response
        .rect;

    paint_accent(ui, rect, theme::ACCENT_RUNNING);
    let _ = rows;
    finish(ui, rect, close_rect, ui.make_persistent_id("watched-panel"), false)
}

/// How many rows [`watched_panel`] will draw, so the window can be sized first.
pub fn panel_rows(watched: &[WatchedRepo]) -> usize {
    if watched.is_empty() {
        return 1;
    }
    let shown = watched.len().min(theme::PANEL_MAX_ROWS);
    let mut accounts = 0;
    let mut last: Option<&str> = None;
    for repo in &watched[..shown] {
        if last != Some(repo.account.as_str()) {
            last = Some(&repo.account);
            accounts += 1;
        }
    }
    let overflow = usize::from(watched.len() > shown);
    shown + accounts + overflow
}

/// One line of the panel: label on the left, optional value right-aligned.
fn panel_row(ui: &mut Ui, text: RichText, right: Option<String>) {
    let right_width = 66.0;
    ui.allocate_ui_with_layout(
        vec2(theme::CONTENT_WIDTH, theme::PANEL_ROW),
        Layout::left_to_right(Align::Center),
        |ui| {
            let label_width = if right.is_some() {
                theme::CONTENT_WIDTH - right_width
            } else {
                theme::CONTENT_WIDTH
            };
            ui.allocate_ui_with_layout(
                vec2(label_width, theme::PANEL_ROW),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add(Label::new(text).truncate());
                },
            );
            if let Some(right) = right {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(Label::new(
                        RichText::new(right)
                            .size(theme::TEXT_SMALL)
                            .color(theme::TEXT_MUTED),
                    ));
                });
            }
        },
    );
}

/// Zero item spacing and a fixed content height, so the card is exactly as tall
/// as [`theme`] promises.
fn prepare(ui: &mut Ui, content_height: f32) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    ui.set_width(theme::CONTENT_WIDTH);
    ui.set_min_height(content_height);
}

/// A single full-width line of truncated text at an exact height.
fn row(ui: &mut Ui, height: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        vec2(theme::CONTENT_WIDTH, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add(Label::new(text).truncate());
        },
    );
}

/// The heading, with the close control tucked into the right-hand end.
/// Returns the close control's rect.
fn title_row(ui: &mut Ui, text: RichText, close_hovered: bool) -> Rect {
    let mut close_rect = Rect::NOTHING;
    ui.allocate_ui_with_layout(
        vec2(theme::CONTENT_WIDTH, theme::ROW_TITLE),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                vec2(theme::CONTENT_WIDTH - CLOSE_SIZE - 6.0, theme::ROW_TITLE),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add(Label::new(text).truncate());
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(CLOSE_SIZE), Sense::hover());
                close_rect = rect;
                paint_close(ui, rect, close_hovered);
            });
        },
    );
    close_rect
}

fn status_row(ui: &mut Ui, view: &RunView, accent: Color32, elapsed: Duration) {
    ui.allocate_ui_with_layout(
        vec2(theme::CONTENT_WIDTH, theme::ROW_STATUS),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                vec2(theme::CONTENT_WIDTH - ELAPSED_WIDTH, theme::ROW_STATUS),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.add(
                        Label::new(
                            RichText::new(status_text(view))
                                .size(theme::TEXT_BODY)
                                .color(status_color(view, accent)),
                        )
                        .truncate(),
                    );
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(Label::new(
                    RichText::new(format_duration(elapsed))
                        .size(theme::TEXT_SMALL)
                        .color(theme::TEXT_MUTED)
                        .monospace(),
                ));
            });
        },
    );
}

/// Register the single card-wide click target and work out what was hit.
fn finish(
    ui: &mut Ui,
    rect: Rect,
    close_rect: Rect,
    id: egui::Id,
    clickable_body: bool,
) -> (Hit, Hovered) {
    let response = ui.interact(rect, id, Sense::click());
    let close_zone = close_rect.expand(CLOSE_SLOP);
    let over_close = |pos: Option<Pos2>| pos.is_some_and(|p| close_zone.contains(p));

    let hovered = Hovered {
        card: response.hovered(),
        close: response.hovered() && over_close(response.hover_pos()),
    };

    let hit = if response.clicked() {
        if over_close(response.interact_pointer_pos()) || !clickable_body {
            Hit::Dismiss
        } else {
            Hit::Open
        }
    } else {
        Hit::None
    };

    if hovered.card {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (hit, hovered)
}

fn shell(hovered: bool) -> egui::Frame {
    let fill = if hovered {
        theme::CARD_BG_HOVER
    } else {
        theme::CARD_BG
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, theme::CARD_BORDER))
        .corner_radius(theme::CARD_CORNER)
        .shadow(egui::epaint::Shadow {
            offset: [0, 3],
            blur: 8,
            spread: 0,
            color: theme::SHADOW,
        })
        .inner_margin(egui::Margin {
            left: theme::PAD_LEFT,
            right: theme::PAD_RIGHT,
            top: theme::PAD_TOP,
            bottom: theme::PAD_BOTTOM,
        })
}

/// The coloured stripe down the left edge, clipped so it keeps the card's own
/// rounded corners instead of poking out of them.
fn paint_accent(ui: &Ui, rect: Rect, accent: Color32) {
    let stripe = Rect::from_min_size(rect.min, vec2(theme::ACCENT_WIDTH, rect.height()));
    ui.painter()
        .with_clip_rect(stripe)
        .rect_filled(rect, theme::CARD_CORNER, accent);
}

/// A hand-drawn X, so the control never depends on a glyph being in the font.
fn paint_close(ui: &Ui, rect: Rect, hovered: bool) {
    let color = if hovered {
        theme::TEXT_CLOSE_HOVER
    } else {
        theme::TEXT_CLOSE
    };
    let stroke = Stroke::new(1.4, color);
    let inner = rect.shrink(4.0);
    let painter = ui.painter();
    if hovered {
        painter.circle_filled(rect.center(), rect.width() * 0.5, theme::TRACK);
    }
    painter.line_segment([inner.left_top(), inner.right_bottom()], stroke);
    painter.line_segment([inner.right_top(), inner.left_bottom()], stroke);
}

fn commit_line(view: &RunView) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !view.branch.is_empty() {
        parts.push(truncate_chars(&view.branch, 28));
    }
    if !view.head_sha.is_empty() {
        parts.push(view.short_sha().to_owned());
    }
    if !view.commit_subject.is_empty() {
        parts.push(truncate_chars(&view.commit_subject, 60));
    }
    parts.join("  \u{b7}  ")
}

fn status_text(view: &RunView) -> String {
    let line = status_line(view);
    match (view.status, view.conclusion) {
        (RunStatus::Completed, Some(conclusion)) => format!("{} {line}", conclusion.glyph()),
        _ => line,
    }
}

fn status_color(view: &RunView, accent: Color32) -> Color32 {
    match view.status {
        RunStatus::Completed => accent,
        _ => theme::TEXT_SECONDARY,
    }
}

/// The estimated-progress bar.
///
/// With duration history we fill `elapsed / median`; without it we sweep a
/// highlight along the track so the card still reads as "working".
fn progress_bar(ui: &mut Ui, view: &RunView, elapsed: Duration, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(theme::CONTENT_WIDTH, theme::BAR_HEIGHT),
        Sense::hover(),
    );
    let radius = theme::BAR_HEIGHT / 2.0;
    let painter = ui.painter();
    painter.rect_filled(rect, radius, theme::TRACK);

    let progress = if view.status == RunStatus::Completed {
        Progress::Fraction(1.0)
    } else {
        history::estimate(elapsed, view.estimate)
    };

    match progress {
        Progress::Fraction(fraction) => {
            if fraction > 0.0 {
                // Keep a sliver visible so a just-started run still reads as
                // having a bar rather than an empty track.
                let width = (rect.width() * fraction).max(theme::BAR_HEIGHT);
                let filled = Rect::from_min_size(rect.min, vec2(width, rect.height()));
                painter.rect_filled(filled, radius, accent);
            }
        }
        Progress::Indeterminate => {
            // A 30%-wide highlight sweeping left to right, once every 1.6s.
            let time = ui.input(|i| i.time) as f32;
            const PERIOD: f32 = 1.6;
            let phase = (time % PERIOD) / PERIOD;
            let band = rect.width() * 0.3;
            let x = rect.left() - band + (rect.width() + band) * phase;
            let band_rect =
                Rect::from_min_size(egui::pos2(x, rect.top()), vec2(band, rect.height()))
                    .intersect(rect);
            if band_rect.width() > 0.5 {
                painter.rect_filled(band_rect, radius, accent.gamma_multiply(0.85));
            }
        }
    }
}
