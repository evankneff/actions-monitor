//! Colours and metrics for the popup. Deliberately a dark, low-contrast palette
//! so the window reads as an overlay rather than a competing application.
//!
//! Every vertical measurement here is exact. Card heights have to be known
//! before anything is drawn - `App::logic` sizes and positions the window while
//! it is still hidden, and no egui pass runs then - so the card layout uses
//! fixed-height rows with item spacing switched off, and the totals below.

use egui::Color32;

/// Width of the whole window, in logical points.
pub const WINDOW_WIDTH: f32 = 356.0;
/// Width of a card; the difference from the window leaves room for the shadow.
pub const CARD_WIDTH: f32 = 340.0;
/// Horizontal inset of a card within the window.
pub const CARD_INSET: f32 = (WINDOW_WIDTH - CARD_WIDTH) / 2.0;
/// Gap between the window and the monitor's work-area corner.
pub const SCREEN_MARGIN: f32 = 16.0;
/// Vertical gap between stacked cards.
pub const CARD_GAP: f32 = 8.0;
/// Padding above the first card and below the last, for shadow bleed.
pub const STACK_PAD: f32 = 8.0;

pub const CARD_CORNER: u8 = 10;
pub const ACCENT_WIDTH: f32 = 3.0;
pub const PAD_LEFT: i8 = 14;
pub const PAD_RIGHT: i8 = 12;
pub const PAD_TOP: i8 = 10;
pub const PAD_BOTTOM: i8 = 11;

/// Inner content width once the card's horizontal padding is removed.
pub const CONTENT_WIDTH: f32 = CARD_WIDTH - PAD_LEFT as f32 - PAD_RIGHT as f32;

// Fixed row heights and the gaps between them.
pub const ROW_TITLE: f32 = 16.0;
pub const ROW_META: f32 = 15.0;
pub const ROW_COMMIT: f32 = 14.0;
pub const ROW_STATUS: f32 = 14.0;
pub const BAR_HEIGHT: f32 = 5.0;
const GAP_TITLE_META: f32 = 2.0;
const GAP_META_COMMIT: f32 = 2.0;
const GAP_COMMIT_STATUS: f32 = 6.0;
const GAP_STATUS_BAR: f32 = 5.0;

pub const GAPS: [f32; 4] = [
    GAP_TITLE_META,
    GAP_META_COMMIT,
    GAP_COMMIT_STATUS,
    GAP_STATUS_BAR,
];

const VERTICAL_PADDING: f32 = PAD_TOP as f32 + PAD_BOTTOM as f32;

/// Content height of a run card.
pub const RUN_CONTENT_HEIGHT: f32 = ROW_TITLE
    + GAP_TITLE_META
    + ROW_META
    + GAP_META_COMMIT
    + ROW_COMMIT
    + GAP_COMMIT_STATUS
    + ROW_STATUS
    + GAP_STATUS_BAR
    + BAR_HEIGHT;

/// Total height of a run card, including its padding.
pub const RUN_CARD_HEIGHT: f32 = RUN_CONTENT_HEIGHT + VERTICAL_PADDING;

/// An account-issue card has the same first three rows and nothing else.
pub const ISSUE_CONTENT_HEIGHT: f32 =
    ROW_TITLE + GAP_TITLE_META + ROW_META + GAP_META_COMMIT + ROW_COMMIT;
pub const ISSUE_CARD_HEIGHT: f32 = ISSUE_CONTENT_HEIGHT + VERTICAL_PADDING;

/// One repository line in the watched-repos panel.
pub const PANEL_ROW: f32 = 15.0;
/// Gap between the panel heading and the first repository.
pub const PANEL_HEAD_GAP: f32 = 5.0;
/// Most repositories the panel will list before summarising the rest.
pub const PANEL_MAX_ROWS: usize = 28;

/// Height of a watched-repos panel listing `rows` repositories.
pub fn panel_height(rows: usize) -> f32 {
    ROW_TITLE
        + PANEL_HEAD_GAP
        + rows as f32 * PANEL_ROW
        + PAD_TOP as f32
        + PAD_BOTTOM as f32
}

pub const TEXT_TITLE: f32 = 13.0;
pub const TEXT_BODY: f32 = 12.0;
pub const TEXT_SMALL: f32 = 11.0;

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    // Color32 stores premultiplied alpha, so scale the channels up front.
    Color32::from_rgba_premultiplied(
        (r as u32 * a as u32 / 255) as u8,
        (g as u32 * a as u32 / 255) as u8,
        (b as u32 * a as u32 / 255) as u8,
        a,
    )
}

pub const CARD_BG: Color32 = rgba(0x1b, 0x1e, 0x23, 0xf2);
pub const CARD_BG_HOVER: Color32 = rgba(0x24, 0x28, 0x2e, 0xf7);
pub const CARD_BORDER: Color32 = rgba(0x3a, 0x40, 0x48, 0xcc);
pub const SHADOW: Color32 = rgba(0x00, 0x00, 0x00, 0x66);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xe8, 0xea, 0xed);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9d, 0xa4, 0xac);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x71, 0x78, 0x80);
pub const TEXT_CLOSE: Color32 = Color32::from_rgb(0x71, 0x78, 0x80);
pub const TEXT_CLOSE_HOVER: Color32 = Color32::from_rgb(0xe8, 0xea, 0xed);

pub const TRACK: Color32 = rgba(0xff, 0xff, 0xff, 0x14);

pub const ACCENT_RUNNING: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xf6);
pub const ACCENT_QUEUED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
pub const ACCENT_SUCCESS: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
pub const ACCENT_FAILURE: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
pub const ACCENT_CANCELLED: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
pub const ACCENT_NEUTRAL: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);

/// Tray icon colour when nothing is happening. Lighter than the queued grey so
/// it stays legible against a dark taskbar.
pub const TRAY_IDLE: Color32 = Color32::from_rgb(0x6b, 0x74, 0x80);

use crate::model::{Conclusion, RunStatus, RunView};

/// The colour that identifies a run's current state, used for the left stripe,
/// the progress bar and the status text.
pub fn accent_for(view: &RunView) -> Color32 {
    match view.status {
        RunStatus::Queued => ACCENT_QUEUED,
        RunStatus::InProgress => ACCENT_RUNNING,
        RunStatus::Completed => match view.conclusion {
            Some(Conclusion::Success) => ACCENT_SUCCESS,
            Some(Conclusion::Failure | Conclusion::TimedOut) => ACCENT_FAILURE,
            Some(Conclusion::Cancelled | Conclusion::ActionRequired) => ACCENT_CANCELLED,
            _ => ACCENT_NEUTRAL,
        },
    }
}

// These are facts about the constants above rather than runtime behaviour, so
// they are checked at compile time: getting one wrong should not build at all.
const _: () = {
    assert!(CARD_WIDTH + 2.0 * CARD_INSET == WINDOW_WIDTH);
    // The drop shadow is drawn outside the card and must not be clipped by the
    // window edge.
    assert!(CARD_INSET >= 4.0);
    assert!(ISSUE_CARD_HEIGHT < RUN_CARD_HEIGHT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_height_grows_one_row_at_a_time() {
        let one = panel_height(1);
        assert_eq!(panel_height(2) - one, PANEL_ROW);
        assert_eq!(panel_height(0), one - PANEL_ROW);
        assert!(one > 0.0);
    }

    #[test]
    fn card_height_is_the_sum_of_its_rows_and_gaps() {
        // The window is sized from these constants before anything is drawn, so
        // they have to agree with the layout in `card.rs`.
        let rows = ROW_TITLE + ROW_META + ROW_COMMIT + ROW_STATUS + BAR_HEIGHT;
        let gaps: f32 = GAPS.iter().sum();
        assert_eq!(RUN_CONTENT_HEIGHT, rows + gaps);
        assert_eq!(
            RUN_CARD_HEIGHT,
            RUN_CONTENT_HEIGHT + PAD_TOP as f32 + PAD_BOTTOM as f32
        );
    }
}
