# actions-monitor — Project Context

Last updated: 2026-09-15

## What This Project Is

A Windows desktop utility that watches one or more GitHub accounts for running
Actions workflows and shows a small, always-on-top popup with live progress
while anything is in flight. When nothing is running it is completely invisible
apart from a tray icon.

It is **not** a CI dashboard, a log viewer, or a GitHub client. It answers one
question — *is my build still going, and how far along is it?* — without making
you leave what you are doing.

**Core philosophy: the app is a glance, not a destination.** Every design
decision serves being ignorable. It never takes focus, never appears in the
taskbar, never asks for attention, and disappears on its own.

## Key Documents

| Document | Purpose |
| --- | --- |
| [prd.md](prd.md) | What the product is, who it is for, and what it deliberately excludes |
| [architecture.md](architecture.md) | Stack, dependency decisions, data models, module layout |
| [mvp.md](mvp.md) | The original minimum scope and its definition of done |
| [coding-style.md](coding-style.md) | Code standards for this repo |
| [changelog.md](changelog.md) | Running summary of changes |
| [../ai/roadmaps/](../ai/roadmaps/) | Phase plans and roadmaps |
| [../README.md](../README.md) | User-facing documentation |

## Current State

**v0.1.0 — feature-complete and in daily use.** 91 unit tests pass, clippy is
clean, and the binary is deployed and running against live GitHub accounts.

Built and verified:

- Multi-account polling with per-account tokens and independent rate budgets
- Conditional requests (ETag / `If-None-Match`) so idle polling is nearly free
- Frameless, transparent, always-on-top popup anchored bottom-left, growing
  upward, that never steals focus and has no taskbar entry
- Popup framing bypasses egui-winit's automatic Windows undecorated shadow;
  idle visibility is checked against the HWND and hidden windows retain size
- Estimated progress bars from locally-learned duration history
- Tray icon whose colour is live status; left-click shows watched repos,
  right-click opens the menu
- `--check` dry run, `--demo` offline replay, autostart install/uninstall
- Config hot-reload, `exclude` patterns, run de-duplication across accounts

The project is being prepared for a public MIT-licensed repository.

## Behavior

- Phase plans and roadmaps live in `ai/roadmaps/` as a **pair** per phase:
  `YYYY-MM-DD-phase-N-name.md` (plan) and `YYYY-MM-DD-roadmap-phase-N-name.md`
  (milestones). Each references the other.
- When a phase completes, move **both** documents to `ai/roadmaps/complete/`.
- Add a short entry to `changelog.md` for each meaningful change, referencing
  the plan doc that drove it.
- Update `Current State` above whenever it stops being true.

Keep documents as living artifacts. Stale docs are worse than no docs.

## What's Explicitly Out of Scope

- Any write access to GitHub. The app only issues `GET` requests.
- Triggering, cancelling, or re-running workflows.
- Log viewing or job output.
- Historical reporting, analytics, or charts.
- Cross-platform support. This is a Windows app and uses Win32 directly.
- Telemetry, crash reporting, or any network call to a host other than
  `api.github.com`.
- A settings GUI. The TOML file is the interface, and it hot-reloads.

## Hard Constraints — Never Violate Without Asking

- **Never log, print, or commit a token.** Errors must name the *account* or the
  *environment variable*, never the secret. There is a test asserting the config
  error path does not echo a token back.
- **Never take focus.** `WS_EX_NOACTIVATE` plus `SW_SHOWNOACTIVATE`. These
  styles must be re-asserted every tick because winit rebuilds the extended
  style from its own flags and silently drops them.
- **Never appear in the taskbar or Alt-Tab.** `WS_EX_TOOLWINDOW`, with
  `WS_EX_APPWINDOW` cleared.
- **Never stay attached to a console** on the long-running path. Closing that
  terminal kills every attached process. Print, then `FreeConsole()`.
- **Never `panic = "abort"`.** A panicking poller task must not take down an app
  that runs for weeks.
- **No `unwrap()` / `expect()` on fallible paths in the steady-state loop.**
  Tests may use `expect` with a message.
- **No network calls outside `api.github.com`.**
- **Do not add a dependency** without checking `architecture.md` first.
- **Card heights must stay compile-time constants.** The window is sized and
  positioned in `App::logic`, before any egui pass runs, so nothing in a card
  may have content-dependent height.

## Code Style

See [coding-style.md](coding-style.md). In short: small focused modules, no
`unwrap` in steady state, `tracing` for all output, clippy clean.

## Architecture

Summary — see [architecture.md](architecture.md) for detail.

- **Concurrency:** a tokio runtime on a dedicated thread, one task per account,
  supervised by a reconciler that starts/stops tasks as config changes.
- **UI/backend boundary:** the backend publishes immutable `Snapshot`s over a
  `tokio::sync::watch` channel and wakes the UI with
  `egui::Context::request_repaint`. The UI diffs each snapshot against the cards
  it is already showing, matched on run id, so cards update in place.
- **Storage:** TOML config and a JSON duration history under
  `%APPDATA%\actions-monitor\`. No database.
- **UI:** eframe/egui with a single frameless transparent viewport, placed and
  sized through raw Win32 calls in physical pixels.

## Design Principles

- Does this help answer "is my build still going?" If no, cut it.
- Would this make the app ask for attention? If yes, rewrite or remove it.
- Could this steal focus, appear in the taskbar, or otherwise interrupt? If yes,
  it is a bug, not a trade-off.
- Does this cost a request when nothing has changed? If yes, use a conditional
  request or do not do it.
- Could this leak a token into a log, an error message, or the UI? If yes, it
  does not ship.

## Development Process

1. Non-trivial work starts with a plan/roadmap pair in `ai/roadmaps/`.
2. Windows-specific behaviour is verified by running the app and inspecting the
   real window — styles, placement and focus are checked from outside the
   process, not assumed.
3. `cargo test` and `cargo clippy --all-targets` must be clean before a change
   is considered done.
4. Update `changelog.md` and this file's `Current State` as work lands.
