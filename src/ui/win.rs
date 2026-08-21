//! Windows-specific window behaviour that winit does not expose directly:
//! never stealing focus, staying off the taskbar, and sitting in the bottom-left
//! corner of the primary monitor's work area in physical pixels.
//!
//! Everything here is a no-op when the handle cannot be resolved, so the app
//! still runs (just with default window behaviour) rather than crashing.

use std::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

/// Where and how big the popup should be, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A monitor's usable area (i.e. excluding the taskbar), in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl WorkArea {
    /// A sane fallback for the (very unlikely) case that Windows will not tell
    /// us about the primary monitor.
    pub const FALLBACK: Self = Self {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };
}

/// Anchor the window to the bottom-left of `work`, growing upward.
///
/// A stack tall enough to overflow the monitor is pinned to the top edge rather
/// than being allowed to run off-screen.
pub fn bottom_left(work: WorkArea, width: i32, height: i32, margin: i32) -> Placement {
    let x = work.left + margin;
    let y = (work.bottom - margin - height).max(work.top);
    Placement {
        x,
        y,
        width,
        height,
    }
}

/// Resolve the raw `HWND` behind an eframe window handle.
pub fn hwnd_of(handle: &impl HasWindowHandle) -> Option<HWND> {
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Apply the extended styles that make this a well-behaved popup:
///
/// * `WS_EX_NOACTIVATE` - the window never becomes the foreground window, so
///   showing it (or clicking a card) never steals focus from what you are doing.
/// * `WS_EX_TOOLWINDOW` - no taskbar button and no Alt-Tab entry.
/// * `WS_EX_TOPMOST` - stays above ordinary windows.
/// * `WS_EX_APPWINDOW` cleared - it is what would force a taskbar button back.
///
/// This has to be re-asserted rather than set once: winit rebuilds the whole
/// extended style from its own flag set whenever something like visibility or
/// window level changes, which silently drops anything we added behind its back.
/// The call is two cheap syscalls and a comparison, so it runs every tick.
///
/// Returns `true` if the styles actually had to be corrected.
pub fn configure(hwnd: HWND) -> bool {
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let wanted = (current
            | WS_EX_NOACTIVATE.0 as isize
            | WS_EX_TOOLWINDOW.0 as isize
            | WS_EX_TOPMOST.0 as isize)
            & !(WS_EX_APPWINDOW.0 as isize);
        if wanted == current {
            return false;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
        true
    }
}

/// Move and resize without activating, keeping the window topmost.
pub fn apply_placement(hwnd: HWND, placement: Placement) {
    unsafe {
        let result = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            placement.x,
            placement.y,
            placement.width,
            placement.height,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
        if let Err(err) = result {
            tracing::debug!("SetWindowPos failed: {err}");
        }
    }
}

/// Show without taking focus. `SW_SHOWNOACTIVATE` is the whole point here:
/// `SW_SHOW` would pull the foreground away from whatever the user is doing.
pub fn show(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

pub fn hide(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

/// The primary monitor's work area, in physical pixels.
///
/// Re-read every frame so a resolution change, a monitor being unplugged or the
/// taskbar moving is picked up without restarting.
pub fn primary_work_area() -> WorkArea {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let RECT {
                left,
                top,
                right,
                bottom,
            } = info.rcWork;
            WorkArea {
                left,
                top,
                right,
                bottom,
            }
        } else {
            WorkArea::FALLBACK
        }
    }
}

/// The window's DPI scale (1.0 at 96 DPI), or `None` if it cannot be read.
pub fn dpi_scale(hwnd: HWND) -> Option<f32> {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    (dpi > 0).then(|| dpi as f32 / 96.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: WorkArea = WorkArea {
        left: 0,
        top: 0,
        right: 2560,
        bottom: 1400, // taskbar occupies the last 40px of a 1440-tall screen
    };

    #[test]
    fn a_single_card_sits_above_the_taskbar_in_the_left_corner() {
        let placement = bottom_left(WORK, 348, 110, 16);
        assert_eq!(placement.x, 16);
        assert_eq!(placement.y, 1400 - 16 - 110);
        assert_eq!(placement.width, 348);
        assert_eq!(placement.height, 110);
    }

    #[test]
    fn the_stack_grows_upward_keeping_its_bottom_edge_fixed() {
        let one = bottom_left(WORK, 348, 110, 16);
        let three = bottom_left(WORK, 348, 350, 16);
        assert_eq!(one.x, three.x, "the left edge never moves");
        assert_eq!(
            one.y + one.height,
            three.y + three.height,
            "the bottom edge never moves"
        );
        assert!(three.y < one.y, "a taller stack starts higher up");
    }

    #[test]
    fn an_overlong_stack_is_pinned_to_the_top_of_the_work_area() {
        let placement = bottom_left(WORK, 348, 5000, 16);
        assert_eq!(placement.y, WORK.top);
    }

    #[test]
    fn a_secondary_monitor_origin_is_respected() {
        // Primary monitors can start at a negative origin when arranged left of
        // another display.
        let work = WorkArea {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        let placement = bottom_left(work, 348, 110, 16);
        assert_eq!(placement.x, -1904);
        assert_eq!(placement.y, 880 - 16 - 110);
    }
}
