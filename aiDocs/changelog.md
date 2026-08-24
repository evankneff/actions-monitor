This is meant to be a CONCISE list of changes to track as we develop this project. When adding to this file, keep comments short and summarized. Always add references back to the source plan docs for each set of changes.

---

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
