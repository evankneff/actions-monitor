This is meant to be a CONCISE list of changes to track as we develop this project. When adding to this file, keep comments short and summarized. Always add references back to the source plan docs for each set of changes.

---

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
