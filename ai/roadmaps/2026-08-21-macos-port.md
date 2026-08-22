# macOS Port — Compile + Non-activating Popup

Created 2026-08-21 · Status: In progress (compiles + tests; visual confirm and two other
items outstanding — see the high-level plan's Phase 5)
Roadmap: [2026-08-21-roadmap-macos-port.md](2026-08-21-roadmap-macos-port.md)

## Goal

Make the popup non-activating on macOS the way `win::configure` already makes it on
Windows: no keyboard focus ever, no click on it activates the process, no Dock/Cmd-Tab
entry. This repo did not compile on macOS at all going in — `windows` was an
unconditional dependency, and `windows-future` 0.3.2 fails to build for a non-Windows
target with `E0425: cannot find type IMarshal in module windows_core::imp`, a bindings
problem unrelated to anything this app actually calls.

## Why this needed more than `with_active(false)`

`viewport()` (`ui/mod.rs`) already sets `with_always_on_top()`, `with_taskbar(false)` and
`with_active(false)` — all correct, none of them sufficient. `WS_EX_NOACTIVATE` is really
two separate things on macOS:

1. Never becomes the key window — winit's `WinitWindow` already implements
   `canBecomeKeyWindow`/`canBecomeMainWindow` returning `true`; nothing flips that from
   the `ViewportBuilder`.
2. A click doesn't activate the *process* — AppKit only grants this to an `NSPanel`
   whose style mask carries `NSWindowStyleMaskNonactivatingPanel`. Setting that bit on a
   plain `NSWindow` (what winit hands back) is silently discarded — no error, no log.
   The window's class has to change at runtime (`object_setClass`).

Established by a standalone spike before any of this repo's code was touched
(`FINDINGS.md`, kept outside this repo), which built a throwaway eframe/winit app and
measured the class-swap requirement directly.

## Approach

**Commit 1 — compile.** `windows`/`winreg` moved to
`[target.'cfg(windows)'.dependencies]`; every direct user (`console.rs`, `autostart.rs`,
`ui/win.rs` and its use in `ui/mod.rs`) cfg-gated. `console::attach`/`detach` are no-ops
on macOS (a Terminal-launched process needs neither: stdout already works, and quitting
Terminal does not `SIGKILL` this process the way closing a Windows console does).
`autostart.rs` gets a placeholder macOS arm — `install`/`uninstall` return a clear
"not yet implemented" error, `status()` returns `Ok(None)` so the tray menu's checkbox
doesn't need special-casing. A real macOS autostart needs a `LaunchAgent` plist, which is
different enough from a registry value to be its own piece of work; parked.

**Commit 2 — the popup.** `ui/mac.rs`, close counterpart of `ui/win.rs`:

- `NonactivatingPanel` (`define_class!`, `NSPanel` subclass, no ivars) — same class swap
  as the Tauri side of this port (SpideySense), same reasoning.
- **KVO.** winit's `WindowDelegate` observes its own window's `effectiveAppearance` via
  KVO, and `object_setClass` past a `NSKVONotifying_…` subclass silently discards the
  observation *registration* while the runtime bookkeeping stays wired — teardown then
  tries to remove an observer that "isn't there" and the resulting exception aborts the
  process (cannot unwind inside `dealloc`). `suspend_winit_kvo`/`resume_winit_kvo` do the
  remove-swap-reinstate dance, in that order, both in `Popup::adopt` and in
  `Popup::restore`. Getting the re-registration's options wrong (`empty()` instead of
  `New | Old`) is a *second*, delayed version of the same abort — triggered by the
  user's next light/dark switch, not by anything this session would have caught by
  accident. `Popup::poke_appearance` forces that switch in-process at startup as a
  standing regression check, so a wrong re-registration aborts immediately rather than
  weeks later.
- `Popup::enforce` — re-asserts style mask / level / collection behaviour. Unlike
  `win::configure`, does not need to run on every visibility change: nothing on macOS's
  show/hide path rebuilds the mask the way winit's Windows backend does on Windows'.
- `Popup::restore`, called from `eframe::App::on_exit` (new impl on `MonitorApp`, macOS
  only) — puts the class back before winit tears the window down, or the process prints
  an `NSAutoreleasePool` double-drain message on exit despite still exiting 0.
- `mac::bottom_left` — anchors to the bottom-left of the primary monitor's work area,
  same policy as `win::bottom_left`, but genuinely simpler: AppKit's origin is already
  bottom-left with y increasing upward, so "grow upward from a fixed bottom edge" needs
  no coordinate inversion (Windows needs one, because its origin is top-left with y
  increasing downward). No DPI scale-factor arithmetic anywhere — AppKit frames are
  already logical, unlike Win32's physical-pixel rects.
- `main.rs` gained the one macOS-only, no-Windows-equivalent piece: `NativeOptions`'s
  `event_loop_builder` hook, setting `ActivationPolicy::Accessory`,
  `with_activate_ignoring_other_apps(false)` (the important one — winit calls
  `activateIgnoringOtherApps:` unconditionally at launch, an *application*-level
  activation no per-window configuration can undo) and `with_default_menu(false)`.

No new *build-blocking* dependency: `objc2`/`objc2-app-kit`/`objc2-foundation` (0.6.4 /
0.3.2, vellum's versions) and `winit` (0.30.13, version-matched to what eframe 0.36.1
already resolves, so cargo unifies rather than compiling two winits) are all genuinely
new here — this repo had no macOS-side Objective-C bridge before. Checked against
`aiDocs/architecture.md`'s dependency table before adding.

## What is verified, and what is not

Verified (lock-independent, no screen needed): `cargo check`/`cargo clippy --all-targets`
clean at every commit boundary, `cargo test` 91/91 (see the roadmap doc for why that
number is unchanged from the documented Windows count), and a `--demo` launch logging:

```
popup window class swapped from=NSKVONotifying_WinitWindow to=ActionsMonitorNonactivatingPanel
KVO appearance self-check passed (dark -> system, no abort)
popup window configured: no focus steal, no taskbar entry (class=NSKVONotifying_ActionsMonitorNonactivatingPanel mask=0x0084 level=25 key=false app_active=false policy=NSApplicationActivationPolicy(1) frontmost="loginwindow")
popup shown cards=1
```

The class-swap and KVO evidence is real *and* stronger than a static read: the appearance
self-check actually forces the exact code path that caused the spike's delayed crash,
and it survived.

**Not verified:**

- Click-without-activation and keystrokes landing elsewhere — needs a screen and a
  mouse. See `MACOS_RUNBOOK.md`.
- The graceful-exit path (`Popup::restore`, the log line `popup window class restored`,
  absence of the `NSAutoreleasePool` message) — only runs when eframe calls `on_exit`,
  which needs the tray's Quit menu item; a `kill` from this session bypasses it entirely
  (no signal handler is registered, so the process just dies, Rust drops and all).

## Follow-up (closed 2026-08-21, commit `9191ed2`)

`paths.rs` resolved everything from `%APPDATA%` unconditionally, which meant the app
compiled and passed its tests but could not actually run on macOS - not just a
window-focus gap, but the difference between "ported" and "usable." Fixed natively
(`$HOME/Library/Application Support/actions-monitor` under `cfg(target_os = "macos")`,
`%APPDATA%` untouched under `cfg(windows)`), deliberately without adding the `dirs`
crate - `aiDocs/architecture.md` had already rejected it for the Windows case on the same
basis. Verified end to end: `env -u APPDATA ./target/debug/actions-monitor --check` now
locates the config directory, and a config-less run writes the first-run template and
exits 0, same as Windows.

## Parked

- **`autostart.rs` macOS arm.** Needs a `LaunchAgent` plist under
  `~/Library/LaunchAgents` and `launchctl` calls, not a `cfg` swap. Currently a clear
  "not implemented" error.
- **Tray + `Accessory` interaction.** Standard configuration, exercised in this session
  (tray icon created successfully in the `--demo` log line `tray icon created`) but not
  interacted with — no mouse.
