This is meant to be a CONCISE list of changes to track as we develop this project. When adding to this file, keep comments short and summarized. Always add references back to the source plan docs for each set of changes.

---

## 2026-09-15 - Popup frame and idle visibility

- Remove the native caption/edge styles behind the card stack. Start the
  viewport hidden with decorations enabled, then convert it to WS_POPUP before
  showing: egui-winit 0.36.1 otherwise enables its Windows shadow and one-pixel
  top edge, ignoring `has_shadow(false)` (macOS only).
- Reconcile visibility with IsWindowVisible instead of the previous tick's
  flag. Keep the last useful size while idle instead of resizing to a one-pixel
  strip, and skip card painting when empty.
- 97 tests pass for the isolated commit; clippy is clean. The full local build
  also passed its 100 tests and formatting checks for the changed UI files.
  Release demo verified running/completed cards, idle, reappearance and forced
  frame/visibility resets: zero stray top-edge pixels and no focus changes.
  Installed and restarted with a backup of the previous executable.
  Plan: [popup frame](../ai/roadmaps/complete/2026-09-15-phase-5-popup-frame.md).

## 2026-09-03 — v0.1.2

- Fix: the app no longer disappears over a sleep/resume. eframe's glow backend
  calls `make_current(..).unwrap()`, so an OpenGL surface invalidated by a resume
  panics the process at 4am and there is nothing left to come back to when the
  machine wakes. The same event can instead just end the winit loop, which is
  indistinguishable from the tray's Quit. `supervisor::respawn` now starts a
  fresh process whenever the event loop ends and `ui::quit_requested()` is false,
  covering both. A strike counter passed down in `ACTIONS_MONITOR_RESPAWN` stops
  the chain after 3 restarts that each died within 60s, and `SM_SHUTTINGDOWN` is
  checked first so signing out does not spawn a process into a dying session.
  Verified in release by posting WM_CLOSE to the demo window: the process was
  replaced 3s later with its arguments intact, and with the strike count
  pre-seeded at 3 it stopped and said so instead.

- Fix: a startup failure was completely silent. The `LogGuard` lived in `run`,
  so the log-writing thread was already shut down by the time `main` wrote its
  `fatal:` line - an invalid config left nothing in the log but
  `actions-monitor starting`, and a `windows_subsystem = "windows"` build has no
  stderr to print to either. The guard now lives in `main`, and
  `console::error_dialog` puts the reason on screen when there is no console.
  Verified with a bad config both ways: the `fatal:` line reached the log file,
  and a launch with no parent console showed a dialog naming the offending line.

## 2026-08-21 — v0.1.1

- Diagnosability: panics are now logged. Release builds are
  `windows_subsystem = "windows"`, so the default hook wrote the message to a
  stderr nobody was attached to and the process vanished leaving no trace -
  which is how the 2026-08-24 exit went unexplained. `logging::install_panic_hook`
  records the message, thread, `file:line` and a backtrace, then chains to the
  previous hook so `--console` still prints. Verified by arming a temporary
  panic in both debug and release and reading the entry back out of the log.

- `run` now logs `event loop ended; actions-monitor is exiting` when
  `run_native` returns `Ok`. A clean return is not proof of a healthy shutdown:
  losing the GL context ends the winit loop exactly like the tray Quit does, so
  an exit line with no "quitting on request from the tray menu" above it is the
  signature of the window being torn down underneath us. Verified with `--demo`
  plus a `taskkill` WM_CLOSE.

  Known gap: `strip = true` and no PDB mean release backtraces show app frames
  as `__ImageBase`. The message and `file:line` survive; the frame names do not.

- A card closed with its X while the run was still going now comes back for 15s
  (`RECALL_LINGER`) once the run finishes, so getting the popup out of the way
  never costs you the result. Closing a card that had already finished is still
  final. Verified with `--demo`: a card closed mid-run reappeared reporting its
  conclusion.

- Fix: Windows 11's compositor drew rounded corners and a pale 1px border around
  the borderless popup, which read as a white box floating around the card stack.
  `win::strip_dwm_frame` now sets `DWMWA_WINDOW_CORNER_PREFERENCE` to
  `DWMWCP_DONOTROUND` and `DWMWA_BORDER_COLOR` to `DWMWA_COLOR_NONE` once at
  startup. Verified by running `--demo` and screenshotting the popup.

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
