This is meant to be a CONCISE list of changes to track as we develop this project. When adding to this file, keep comments short and summarized. Always add references back to the source plan docs for each set of changes.

---

## 2026-08-21 — macOS port (compile + non-activating popup)

See `ai/roadmaps/2026-08-21-macos-port.md`. Two commits:

- **Compile.** `windows`/`winreg` did not even build for a non-Windows target
  (`windows-future` 0.3.2 fails with `E0425` in `windows_core::imp`, unrelated to
  anything this app calls). Moved both to `[target.'cfg(windows)'.dependencies]` and
  cfg-gated every direct user: `console.rs` (no-op `attach`/`detach` on macOS - a
  Terminal-launched process needs neither), `autostart.rs` (Windows Run-key impl stays;
  macOS needs a `LaunchAgent`, parked as its own piece of work - `install`/`uninstall`
  now return a clear error there instead of failing to compile), `ui/win.rs`.
- **The popup.** New `ui/mac.rs`. Same problem as SpideySense's Tauri port: a click
  activating the process has no macOS style flag, only an `NSPanel` with
  `NSWindowStyleMaskNonactivatingPanel` in its mask, which AppKit silently refuses on a
  plain window - so the live `NSWindow` is swapped to a custom `NSPanel` subclass at
  runtime. The extra hazard here that SpideySense's Tauri/tao stack doesn't have:
  winit's own window observes its `effectiveAppearance` via KVO, and the class swap
  silently discards that registration's bookkeeping while leaving the registration
  itself in place - unless undone and redone around the swap (in that order), the
  process aborts on exit, or on the user's first light/dark switch if the
  re-registration options are wrong. Both are guarded against and self-checked at
  startup (`Popup::poke_appearance` forces an appearance change immediately rather than
  waiting for a real one).
- `main.rs` gained `NativeOptions::event_loop_builder` - the one macOS-only setting with
  no Windows counterpart, since winit calls `activateIgnoringOtherApps:` unconditionally
  at launch and no per-window fix can undo an application-level activation.
- `mac::bottom_left` anchors the same way `win::bottom_left` does, but is simpler:
  AppKit's bottom-left, y-up origin means "grow upward from a fixed bottom" needs no
  coordinate inversion, and there is no DPI scale-factor arithmetic at all (AppKit
  frames are already logical points).
- Verified: `cargo test` 91/91 (matches the documented Windows count - 4 Windows-only
  `win.rs` tests dropped on this target, 4 new `mac.rs` tests added), `cargo clippy
  --all-targets` clean, and a `--demo` launch logs the class swap, a real in-process KVO
  self-check passing, and `class=…ActionsMonitorNonactivatingPanel mask=0x0084 level=25
  policy=Accessory`. **Not verified: click-without-activation (needs a screen and a
  mouse) or the graceful-exit path (needs the tray's Quit item; this session's `kill`
  bypassed it entirely).** See `MACOS_RUNBOOK.md`.
- Parked, found but out of scope for this change: `paths.rs` hardcodes `%APPDATA%` and
  blocks even `--demo` on macOS without a manual env var override.

## 2026-08-21 — v0.1.1

- Fix: the app was killed whenever the terminal it was launched from was closed.
  It called `AttachConsole(ATTACH_PARENT_PROCESS)` on the normal launch path, and
  Windows terminates every process attached to a console when that window closes.
  It now detaches with `FreeConsole()` before entering the event loop, except
  under `--console`. Verified by launching from a console and closing it.

## 2026-08-21 — v0.1.0

Initial build. See `ai/roadmaps/2026-08-21-high-level-project-plan.md`.

- Popup window: frameless, transparent, always-on-top, bottom-left of the primary
  work area, grows upward, never takes focus, no taskbar entry.
- Multi-account polling with per-account tokens, conditional requests (ETag),
  active/idle cadences, rate-limit backoff and per-repo error isolation.
- Estimated progress bars from locally-learned duration history.
- Tray icon with live status colour; left-click shows watched repos, right-click
  opens the menu (reload, open config, open logs, autostart toggle, quit).
- `--check` dry run, `--demo` offline replay, autostart install/uninstall,
  config hot-reload, `exclude` patterns.
- Prepared for public release: MIT licence, .gitignore, package metadata,
  test fixtures scrubbed of real tokens and organisation names.

Fixes found by running it rather than reasoning about it:

- winit rebuilds `GWL_EXSTYLE` on visibility changes, silently dropping
  `WS_EX_NOACTIVATE`/`WS_EX_TOOLWINDOW`. Now re-asserted every tick.
- Console handles were being overwritten, so `--install-autostart` printed
  nothing and `--help > file` was broken.
- A repo watched by two accounts produced two identical cards; runs are now
  de-duplicated on `(repo, run_id)`.
- Autostart would open an editor at every login until the config was filled in.
