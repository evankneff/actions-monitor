//! macOS counterpart of `win.rs`. Same job — never steal focus, no taskbar/Dock entry,
//! sit anchored to a corner of the primary monitor's work area — different mechanism at
//! almost every step. See that file's doc comment for the Windows side of each mapping:
//!
//! | Windows                        | macOS                                                    |
//! |---------------------------------|----------------------------------------------------------|
//! | `WS_EX_NOACTIVATE`              | class is an `NSPanel` **and** `NSWindowStyleMaskNonactivatingPanel` |
//! | `WS_EX_TOOLWINDOW`               | `NSApplicationActivationPolicy::Accessory` (whole app, not the window) |
//! | `WS_EX_TOPMOST`                  | `setLevel:` above `kCGNormalWindowLevel`                  |
//! | `SW_SHOWNOACTIVATE`              | `orderFrontRegardless`                                    |
//! | `SetWindowPos(SWP_NOACTIVATE)`   | `setFrame:display:` — AppKit has no activating move        |
//!
//! **The style bit alone is not enough.** `NSWindowStyleMaskNonactivatingPanel` does not
//! merely do nothing on a plain `NSWindow` — it does not stick. `setStyleMask:` accepts
//! it and `styleMask` reads back without it, with no error and no exception. So the
//! window has to *be* a panel, which means changing the class of a live object at
//! runtime — see [`Popup::adopt`].
//!
//! **The taskbar row is not per-window.** macOS decides Dock/Cmd-Tab/menu-bar presence
//! from the *application's* activation policy, set once in `main.rs`'s
//! `event_loop_builder` hook (belt) and again in [`Popup::apply_once`] (braces).

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{define_class, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSPanel, NSResponder, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowLevel, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSKeyValueObservingOptions, NSObject, NSObjectNSKeyValueObserverRegistration, NSPoint,
    NSRect, NSSize, NSString,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// `NSStatusWindowLevel`. Where the menu-bar extras live, and the closest macOS
/// equivalent of `HWND_TOPMOST` for a notification-shaped popup.
const POPUP_LEVEL: NSWindowLevel = 25;

/// Added to whatever winit put in the mask, never replacing it — the same shape as the
/// Windows `current | WS_EX_NOACTIVATE`. winit owns `Borderless`/`Resizable` and derives
/// them from the `ViewportBuilder`, so overwriting the mask wholesale would fight it.
const WANTED_MASK: NSWindowStyleMask = NSWindowStyleMask::NonactivatingPanel;

/// Spaces and full-screen behaviour: follow the user between Spaces, be allowed over
/// another app's full-screen window (without this the popup cannot appear over one at
/// all — on a laptop that is most of the time), do not slide with Mission Control, stay
/// out of Cmd-` cycling.
const WANTED_BEHAVIOR: NSWindowCollectionBehavior = NSWindowCollectionBehavior(
    NSWindowCollectionBehavior::CanJoinAllSpaces.0
        | NSWindowCollectionBehavior::FullScreenAuxiliary.0
        | NSWindowCollectionBehavior::Stationary.0
        | NSWindowCollectionBehavior::IgnoresCycle.0,
);

define_class!(
    /// An `NSPanel` that refuses to become key or main, whatever anyone asks of it.
    ///
    /// The style bit alone stops AppKit from *activating the app* on a click. It does
    /// not stop deliberate focus: `-makeKeyAndOrderFront:` on a non-activating panel
    /// still makes it the key window, and winit's `set_visible(true)` calls exactly
    /// that. Overriding `canBecomeKeyWindow` is the belt to that braces — a stray
    /// `ViewportCommand::Visible(true)` degrades to "nothing happens" instead of "the
    /// user's typing goes into our webview".
    ///
    /// Adds **no instance variables**, which is the precondition [`Popup::adopt`]'s
    /// class swap relies on.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ActionsMonitorNonactivatingPanel"]
    #[derive(Debug)]
    struct NonactivatingPanel;

    impl NonactivatingPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

/// The key path winit key-value-observes on its window, hard-coded here because it has
/// to be — see [`suspend_winit_kvo`].
const WINIT_KVO_KEY_PATH: &str = "effectiveAppearance";

/// Un-register winit's KVO observation, returning the observer so it can be put back.
///
/// **Not optional, and the failure mode is a `SIGABRT`.** Registering a KVO observer
/// makes the runtime replace the object's class with a generated `NSKVONotifying_…`
/// subclass — a real run reports the starting class as `NSKVONotifying_WinitWindow`, not
/// `WinitWindow`. `object_setClass` past that subclass throws KVO's bookkeeping away
/// while leaving the *observation registration* in place, and nothing complains until
/// teardown: winit's `WindowDelegate::drop` calls `removeObserver:forKeyPath:`, AppKit
/// raises "not registered as an observer", `objc2` turns the exception into a Rust
/// panic, and because the panic is inside an `extern "C"` `dealloc` it cannot unwind —
/// the process aborts. Measured, not theorised; see the macOS window spike's
/// `FINDINGS.md` §4.6 for the backtrace.
///
/// Removing the observation first lets KVO restore the class to plain `WinitWindow` on
/// its own, so the swap operates on the class winit actually declared, and re-adding
/// afterwards (see [`resume_winit_kvo`]) lets KVO build a fresh
/// `NSKVONotifying_ActionsMonitorNonactivatingPanel` *underneath* our override. winit's
/// teardown then finds the registration it expects.
///
/// # What this does *not* preserve
///
/// winit's is not the only observation on the window — AppKit's own private
/// `_windowLayerContext` is also there and is severed for the life of the window, with
/// no supported way to re-bind it. Nothing observably broke in the spike this was
/// modelled on, but it is an unproven assumption, not a guarantee.
///
/// Returns `None` if the window has no delegate, in which case there is no observation
/// to preserve.
fn suspend_winit_kvo(window: &NSWindow) -> Option<Retained<NSObject>> {
    let delegate = window.delegate()?;
    // winit's observer *is* its window delegate: `WindowDelegate::drop` passes `self`,
    // and the same object is installed with `setDelegate:`.
    let observer: Retained<NSObject> = unsafe { Retained::cast_unchecked(delegate) };
    // SAFETY: pairs with the `addObserver:` in `resume_winit_kvo`, and winit is the only
    // other party observing this key path on this window.
    unsafe { window.removeObserver_forKeyPath(&observer, &NSString::from_str(WINIT_KVO_KEY_PATH)) };
    Some(observer)
}

/// Put winit's observation back, **with the options winit registered it with**.
///
/// `New | Old` is not a guess. winit's `observeValueForKeyPath:` (winit 0.30.13,
/// `platform_impl/macos/window_delegate.rs:448`) reaches into the change dictionary with
/// `.expect("requested change dictionary did not contain `NSKeyValueChangeOldKey`")`.
/// Re-register with `empty()` and everything looks fine until the *first light/dark
/// switch*, at which point that `expect` fires inside an Objective-C callback, cannot
/// unwind, and aborts the process — a delayed crash the spike this is modelled on
/// reproduced by A/B and is why [`Popup::adopt`] calls [`Popup::poke_appearance`] once at
/// startup as a standing regression check, rather than waiting for the user's first
/// appearance change to find out.
fn resume_winit_kvo(window: &NSWindow, observer: &NSObject) {
    // SAFETY: `observer` outlives the window — it is the window's own delegate,
    // retained by AppKit — and winit will remove this registration in
    // `WindowDelegate::drop`. Context stays null to match what the dump of a live
    // registration shows (`Context: 0x0`); `removeObserver:forKeyPath:` matches on the
    // observer/key-path pair, and winit's teardown uses the context-free variant.
    unsafe {
        window.addObserver_forKeyPath_options_context(
            observer,
            &NSString::from_str(WINIT_KVO_KEY_PATH),
            NSKeyValueObservingOptions::New | NSKeyValueObservingOptions::Old,
            std::ptr::null_mut(),
        );
    }
}

/// A monitor's usable area in points — the `visibleFrame` equivalent of `win::WorkArea`,
/// but with AppKit's bottom-left, y-up origin instead of Win32's top-left, y-down one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Where and how big the popup should be, in points. The macOS counterpart of
/// `win::Placement` — no scale factor anywhere, because AppKit frames are already
/// logical (see `win::Placement`'s doc comment on why Windows needs one and this
/// doesn't).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Anchor the window to the bottom-left of `work`, growing upward — the same policy as
/// `win::bottom_left`, translated to AppKit's coordinate system.
///
/// This is the one place the port genuinely gets simpler rather than just different:
/// AppKit's origin is already bottom-left with y increasing upward, so "anchor the
/// bottom, grow upward" is just "keep `y` constant" — no inversion needed, unlike
/// Windows, where the top-left/y-down convention means the *top* edge has to be
/// recomputed as `height` changes to hold the bottom edge fixed.
///
/// A stack tall enough to overflow the monitor is pinned flush to the top edge (no
/// margin there, matching `win::bottom_left`'s clamp to `work.top` exactly) rather than
/// being allowed to run off the top of the screen.
pub fn bottom_left(work: WorkArea, width: f64, height: f64, margin: f64) -> Placement {
    let top = work.y + work.height;
    let y = if work.y + margin + height > top {
        top - height
    } else {
        work.y + margin
    };
    Placement {
        x: work.x + margin,
        y,
        width,
        height,
    }
}

/// A live handle on the popup window. Cheap to clone-free; hold one for the process
/// lifetime. `Retained<NSWindow>` is `!Send`, which is fine — `MonitorApp` lives on the
/// main thread — but means this can never go into an `Arc`/`Mutex`.
pub struct Popup {
    window: Retained<NSWindow>,
    /// The class the window had before [`Popup::adopt`] swapped it, so [`Popup::restore`]
    /// can put it back. Classes are never freed, so `'static` is honest.
    original_class: &'static AnyClass,
}

impl Popup {
    /// Turn the window behind `handle` into a non-activating panel.
    ///
    /// Call this once, as early as the native handle exists — from
    /// `eframe::CreationContext`, the same place `win::hwnd_of` is already called from.
    ///
    /// # The class swap, and why it is sound here
    ///
    /// `object_setClass` needs three things: the new class must be a subclass of the
    /// old (in practice: same instance layout), must add no ivars, and must override
    /// nothing incompatibly.
    ///
    /// 1. **Layout.** `NonactivatingPanel` descends from `NSPanel`; the live object's
    ///    class is winit's `WinitWindow`, which descends from `NSWindow`. They are
    ///    siblings, not a subclass relationship, but `NSPanel` has added no ivars over
    ///    `NSWindow` in any shipping AppKit, so the instance sizes match. This is the
    ///    same manoeuvre the `tauri-nspanel` crate is built on.
    /// 2. **Overridden methods.** `WinitWindow` (winit 0.30.13,
    ///    `platform_impl/macos/window.rs`) declares exactly two methods —
    ///    `canBecomeKeyWindow`/`canBecomeMainWindow`, both `true` — and **no ivars at
    ///    all**; every piece of winit's window state lives on the separate
    ///    `WindowDelegate` object. So the swap discards two methods whose answers we
    ///    were inverting anyway, and nothing else. **Re-check this fact specifically on
    ///    a winit upgrade** — it is what makes this safe for this winit version.
    ///
    /// `AnyObject::set_class` carries a `debug_assert_eq!` on the two instance sizes, so
    /// a debug build of this binary *is* the check: if AppKit or winit ever changes
    /// layout, it aborts here rather than corrupting a live window.
    ///
    /// Returns `None` if the handle is not an AppKit one or has no window yet — same
    /// shape as `win::hwnd_of`, where an unresolvable handle degrades to default window
    /// behaviour rather than panicking.
    pub fn adopt(handle: &impl HasWindowHandle, mtm: MainThreadMarker) -> Option<Self> {
        let RawWindowHandle::AppKit(appkit) = handle.window_handle().ok()?.as_raw() else {
            return None;
        };

        // The AppKit variant of `RawWindowHandle` carries the *view*, not the window;
        // the window is one hop up. (On Windows the handle is the window, which is why
        // `win::hwnd_of` has no equivalent hop.)
        //
        // SAFETY: `raw-window-handle`'s contract is that `ns_view` points at a live
        // `NSView` for at least as long as the borrow of `handle`, and `NSView` is
        // main-thread-only, which `mtm` witnesses.
        let view: &NSView = unsafe { &*appkit.ns_view.as_ptr().cast::<NSView>() };
        let window = view.window()?;

        // KVO first — see [`suspend_winit_kvo`] for why skipping this aborts the
        // process on exit.
        let observer = suspend_winit_kvo(&window);

        let previous = {
            let object: &AnyObject = &window;
            // SAFETY: the two preconditions are argued in this function's doc comment;
            // the third (no added ivars) holds because `NonactivatingPanel` declares
            // none.
            unsafe { AnyObject::set_class(object, NonactivatingPanel::class()) }
        };
        tracing::info!(
            from = %previous.name().to_string_lossy(),
            to = %window.class().name().to_string_lossy(),
            "popup window class swapped"
        );

        if let Some(observer) = observer {
            resume_winit_kvo(&window, &observer);
        }

        let popup = Self {
            window,
            original_class: previous,
        };
        popup.apply_once(mtm);
        popup.enforce();
        popup.poke_appearance(mtm);
        Some(popup)
    }

    /// The settings that only need saying once, because nothing in winit touches them.
    fn apply_once(&self, mtm: MainThreadMarker) {
        // The app-wide half of `WS_EX_TOOLWINDOW`: no Dock icon, no Cmd-Tab entry, no
        // menu bar of our own. `Accessory` rather than `Prohibited` because prohibited
        // apps cannot be activated at all, which would break the tray icon's own menu.
        // `main.rs`'s `event_loop_builder` hook sets this itself, earlier, from
        // `applicationDidFinishLaunching:`; this call is the belt to that braces.
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        unsafe {
            // Without this the popup vanishes whenever the user clicks another app —
            // which, for a window that deliberately never activates, is always.
            self.window.setHidesOnDeactivate(false);
            self.window.setExcludedFromWindowsMenu(true);

            // `NSPanel`-only properties, legal now that the class swap has happened.
            let panel: &NSPanel = &*(&*self.window as *const NSWindow).cast::<NSPanel>();
            panel.setFloatingPanel(true);
            panel.setBecomesKeyOnlyIfNeeded(true);
        }
    }

    /// Re-assert the properties winit rebuilds behind our back. Returns `true` if it had
    /// to correct something.
    ///
    /// The macOS counterpart of `win::configure`. In winit 0.30.13
    /// (`platform_impl/macos/window_delegate.rs`) `set_style_mask` **replaces** the mask
    /// rather than merging into it, reached from `set_decorations`, `set_resizable`,
    /// `set_fullscreen`, `set_simple_fullscreen` and `set_content_protected`; `setLevel:`
    /// is likewise overwritten by `set_window_level` and both fullscreen paths. Unlike
    /// Windows, **none of that is on the show/hide path** — a popup that is only shown,
    /// hidden and moved never touches those setters, so this does not need to run every
    /// tick the way `win::configure` does. Called after the swap and safe to call again
    /// any time; cheap either way (two message sends, three comparisons).
    pub fn enforce(&self) -> bool {
        let mut corrected = false;

        let mask = self.window.styleMask();
        if !mask.contains(WANTED_MASK) {
            self.window.setStyleMask(mask | WANTED_MASK);
            corrected = true;
        }
        if self.window.level() != POPUP_LEVEL {
            self.window.setLevel(POPUP_LEVEL);
            corrected = true;
        }
        if self.window.collectionBehavior() != WANTED_BEHAVIOR {
            self.window.setCollectionBehavior(WANTED_BEHAVIOR);
            corrected = true;
        }

        corrected
    }

    /// Put the window back the way winit built it, before winit tears it down.
    ///
    /// Call from `eframe::App::on_exit`, which runs while the window and its delegate
    /// are still alive. Not a `Drop` impl: the drop order of `MonitorApp` relative to
    /// eframe's own window teardown is not this code's to decide, and this has to
    /// happen first.
    ///
    /// Without it the process still exits 0, but AppKit prints `-[NSAutoreleasePool
    /// drain]: This pool has already been drained` on the way out — bisected (in the
    /// spike this is modelled on) to the class swap itself, not to any property set
    /// alongside it. Restoring the class before teardown removes it.
    pub fn restore(&self) {
        // Same ordering as `adopt`, in reverse: KVO first, or winit's
        // `removeObserver:` at teardown lands on a registration that is no longer
        // there.
        let Some(observer) = suspend_winit_kvo(&self.window) else {
            // No delegate means no observation to preserve, and also no safe way to
            // re-register. Leaving the class swapped is the lesser evil - a stray
            // console line, not an abort.
            tracing::warn!("no delegate at shutdown; leaving the popup window class swapped");
            return;
        };
        let object: &AnyObject = &self.window;
        // SAFETY: `original_class` is the class this very object had a moment ago, so
        // every precondition that held for the swap holds for the swap back.
        unsafe { AnyObject::set_class(object, self.original_class) };
        resume_winit_kvo(&self.window, &observer);
        tracing::info!(
            class = %self.window.class().name().to_string_lossy(),
            "popup window class restored"
        );
    }

    /// Force `effectiveAppearance` to change, in-process, so winit's KVO observer
    /// actually fires — a standing regression check for [`resume_winit_kvo`]'s options,
    /// rather than waiting for the user's first light/dark switch to find out whether
    /// they were right. Touches nothing outside this process and does not alter the
    /// user's system theme. If the options were ever wrong, this aborts the process at
    /// startup instead of some unrelated moment weeks later.
    fn poke_appearance(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        let dark = unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) };
        app.setAppearance(dark.as_deref());
        app.setAppearance(None);
        tracing::debug!("KVO appearance self-check passed (dark -> system, no abort)");
    }

    /// Show without taking focus.
    ///
    /// `orderFrontRegardless` is the whole point, and the counterpart of
    /// `SW_SHOWNOACTIVATE`. Not `set_visible(true)` (what egui's
    /// `ViewportCommand::Visible` would reach) — that is `makeKeyAndOrderFront:`, which
    /// the `canBecomeKeyWindow` override neuters but is still the wrong call to reach
    /// for. Not `orderFront:` either: it is a no-op while the application is inactive,
    /// which for this app is always, so the popup would simply never appear.
    pub fn show(&self) {
        self.window.orderFrontRegardless();
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    /// The primary monitor's work area, in points. The macOS counterpart of
    /// `win::primary_work_area`; re-read every call for the same reason (a resolution
    /// change or a monitor being unplugged is picked up without a restart).
    ///
    /// `NSScreen::screens()[0]` is documented as the screen containing the menu bar,
    /// i.e. the primary display — the direct analogue of Windows'
    /// `MONITOR_DEFAULTTOPRIMARY`. Deliberately not `NSScreen::mainScreen`, which means
    /// "the screen holding the key window": this app never has one, so that would
    /// answer with whatever *other* app happens to be focused.
    pub fn work_area(&self, mtm: MainThreadMarker) -> Option<WorkArea> {
        let screen = NSScreen::screens(mtm)
            .firstObject()
            .or_else(|| NSScreen::mainScreen(mtm))?;
        let visible = screen.visibleFrame();
        Some(WorkArea {
            x: visible.origin.x,
            y: visible.origin.y,
            width: visible.size.width,
            height: visible.size.height,
        })
    }

    /// Move and resize without activating, in points. `display: false` — the popup may
    /// well be hidden right now, and forcing a display pass on an off-screen window is
    /// wasted work. AppKit draws it when it is ordered in.
    pub fn place(&self, placement: Placement, _mtm: MainThreadMarker) {
        self.window.setFrame_display(
            NSRect::new(
                NSPoint::new(placement.x, placement.y),
                NSSize::new(placement.width, placement.height),
            ),
            false,
        );
    }

    /// Everything an automated check can observe about focus configuration, in one
    /// line. This is *configuration*, not *behaviour* — it cannot observe whether a
    /// click actually failed to steal focus, only that the class swap and style bits
    /// took effect.
    pub fn diagnostics(&self, mtm: MainThreadMarker) -> String {
        let app = NSApplication::sharedApplication(mtm);
        let frontmost = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|a| a.localizedName())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "<unknown>".into());
        format!(
            "class={} mask={:#06x} level={} key={} app_active={} policy={:?} frontmost={frontmost:?}",
            self.window.class().name().to_string_lossy(),
            self.window.styleMask().0,
            self.window.level(),
            self.window.isKeyWindow(),
            app.isActive(),
            app.activationPolicy(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: WorkArea = WorkArea {
        x: 0.0,
        y: 0.0,
        width: 2560.0,
        height: 1400.0, // menu bar and Dock already excluded, like visibleFrame is
    };

    #[test]
    fn a_single_card_sits_above_the_dock_in_the_left_corner() {
        let placement = bottom_left(WORK, 348.0, 110.0, 16.0);
        assert_eq!(placement.x, 16.0);
        // Unlike win::bottom_left, the window's own y *is* the bottom edge - no
        // "bottom - margin - height" arithmetic needed, because AppKit's origin is
        // already bottom-left.
        assert_eq!(placement.y, 16.0);
        assert_eq!(placement.width, 348.0);
        assert_eq!(placement.height, 110.0);
    }

    #[test]
    fn the_stack_grows_upward_keeping_its_bottom_edge_fixed() {
        let one = bottom_left(WORK, 348.0, 110.0, 16.0);
        let three = bottom_left(WORK, 348.0, 350.0, 16.0);
        assert_eq!(one.x, three.x, "stacking must not move horizontally");
        assert_eq!(one.y, three.y, "the bottom edge (== y, in this coordinate system) never moves");
    }

    #[test]
    fn an_overlong_stack_is_pinned_flush_to_the_top_of_the_work_area() {
        // Mirrors win::bottom_left's "pinned to the top" test exactly: when the stack
        // would run off-screen, the top edge is held flush (no margin), same as
        // Windows' clamp to `work.top` with no margin subtracted.
        let placement = bottom_left(WORK, 348.0, 5000.0, 16.0);
        assert_eq!(placement.y + placement.height, WORK.height);
    }

    #[test]
    fn a_secondary_monitor_origin_is_respected() {
        let work = WorkArea {
            x: -1920.0,
            y: -200.0,
            width: 1920.0,
            height: 1080.0,
        };
        let placement = bottom_left(work, 348.0, 110.0, 16.0);
        assert_eq!(placement.x, -1920.0 + 16.0);
        assert_eq!(placement.y, -200.0 + 16.0);
    }
}
