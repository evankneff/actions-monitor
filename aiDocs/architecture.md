# actions-monitor — Architecture

Version: 1.0
Last updated: 2026-08-21
Scope: v0.1.0 as built
Package versions verified: 2026-08-21 (resolved by `cargo add` against crates.io)

## Platform & Build

| Layer | Decision | Notes |
| --- | --- | --- |
| Language | Rust, 2024 edition | `rust-version = "1.85"` |
| Target | `x86_64-pc-windows-msvc` | Win32 APIs used directly; not portable |
| Subsystem | `windows` in release, `console` in debug | `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` so nothing flashes at login |
| Release profile | `opt-level = "s"`, `lto`, `codegen-units = 1`, `strip` | ~8.8MB single binary |
| Panic strategy | **unwind** (default) | Deliberate: `panic = "abort"` would let one bad task kill an app that runs for weeks |

## Dependencies

### Core

| Package | Version | Purpose |
| --- | --- | --- |
| `tokio` | 1.53 | Async runtime; one task per account |
| `reqwest` | 0.13 | HTTP, `rustls` feature (no OpenSSL on Windows) |
| `serde` / `serde_json` | 1.0 | GitHub API types, history file |
| `toml` | 1.1 | Config parsing |
| `chrono` | 0.4 | API timestamps (`serde` feature) |
| `eframe` / `egui` | 0.36 | UI, `glow` renderer |
| `windows` | 0.62 | Win32: window styles, placement, DPI, console |
| `raw-window-handle` | 0.6 | Resolves eframe's window to an `HWND` |
| `tray-icon` | 0.24 | Notification-area icon and menu (bundles `muda`) |
| `notify` | 8.2 | Config file watching |
| `winreg` | 0.56 | Autostart `Run` key |
| `open` | 5.4 | Launching the browser / editor |
| `tracing` + `-subscriber` + `-appender` + `-log` | — | Logging to a rolling daily file |
| `anyhow` / `thiserror` | — | Application vs typed library errors |

### Deliberately not used

| Package | Why not |
| --- | --- |
| `clap` | Six flags; hand-rolled parsing is ~25 lines and one fewer dependency tree |
| `dirs` / `directories` | `%APPDATA%` via `std::env::var_os` is sufficient |
| `image` | The tray icon is generated as raw RGBA at runtime; no decoder needed |
| `octocrab` | We use four endpoints; a full client is more surface than value |
| native-tls / OpenSSL | rustls avoids the Windows build pain entirely |

## Key API Decisions

### eframe 0.36 — `App` is split

`App::logic(&mut self, ctx, frame)` runs on **every tick, including while the
window is hidden** (eframe runs no egui pass then). `App::ui(&mut self, ui, frame)`
runs only when painting.

This maps exactly onto the app: `logic` ingests snapshots, sizes and places the
window, and shows/hides it; `ui` only draws.

**Consequence — the important one:** the window is sized *before* any egui pass,
so card heights cannot be measured. They are compile-time constants in
`ui::theme`, and every card row is allocated at an exact height with
`item_spacing` set to zero. `const _: () = assert!(...)` blocks guard the
arithmetic. Anything with content-dependent height breaks this.

### winit rebuilds extended window styles

winit recomputes the whole `GWL_EXSTYLE` from its own flag set whenever
visibility or window level changes, silently dropping anything set behind its
back. `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` therefore must be **re-asserted
every tick** (`ui::win::configure`, called from `logic`). Setting them once at
startup is not enough and was an actual bug.

Because Windows only re-evaluates taskbar presence when a window is shown, if
the styles are corrected while visible the window is hidden and re-shown.

### Positioning is done in Win32, not egui

`SetWindowPos(..., SWP_NOACTIVATE)` with physical pixels from
`MonitorFromPoint` + `GetMonitorInfoW` (`rcWork`), scaled by `GetDpiForWindow`.
This sidesteps egui's points-vs-pixels ambiguity across monitors and is the only
way to get a non-activating move. Visibility uses `SW_SHOWNOACTIVATE`.

### Conditional requests

Every GET stores the response `ETag` and replays it as `If-None-Match`.
GitHub answers `304 Not Modified` for unchanged resources, and **304s do not
count against the rate limit** — this is what makes 60-second polling of dozens
of repos affordable. The cache holds the raw body so a 304 can be re-parsed.

### tray-icon threading

The tray must be created on the thread owning the message loop — for eframe,
the thread running `App::new` and `App::logic`. Its event handlers fire from
inside the window procedure, so they queue a command **and** call
`Context::request_repaint`, otherwise a click would not be acted on until the
next scheduled tick.

Only `MouseButtonState::Up` is handled; `Down` and `Up` both arrive and acting on
both would toggle twice per click.

### Console attachment ends the process when the terminal closes

Windows terminates **every process attached to a console** when that console
window is closed. A GUI-subsystem app that calls
`AttachConsole(ATTACH_PARENT_PROCESS)` to print a message has therefore tied its
own lifetime to the terminal it was launched from.

So the long-running path attaches only long enough to print first-run guidance,
then calls `FreeConsole()` before entering the event loop. `--console` is the
deliberate exception, since that mode exists to watch the log live.

## Data Models

### `Snapshot` — backend to UI, over `tokio::sync::watch`

| Field | Type | Notes |
| --- | --- | --- |
| `runs` | `Vec<RunView>` | De-duplicated across accounts by `(repo, run_id)` |
| `issues` | `Vec<AccountIssue>` | Accounts that stopped polling |
| `watched` | `Vec<WatchedRepo>` | Every repo being polled, with last-checked time |
| `linger` | `Duration` | Mirrored from config so the UI needs no config handle |

### `RunView`

| Field | Type | Notes |
| --- | --- | --- |
| `key` | `RunKey { account, run_id }` | Account-scoped so two accounts cannot collide |
| `repo`, `workflow`, `run_number` | `String` / `u64` | Card heading |
| `branch`, `head_sha`, `commit_subject` | `String` | Third line; subject is the first line only |
| `status`, `conclusion` | enums | GitHub's several pre-run states fold into `Queued` |
| `started_at` | `DateTime<Utc>` | `run_started_at`, falling back to `created_at` |
| `current` | `Option<JobProgress>` | Live job/step line |
| `estimate` | `Option<Duration>` | Median from history |
| `finished_at` | `Option<Instant>` | When *we* observed completion; drives linger |

### History file — `%APPDATA%\actions-monitor\history.json`

`{ "owner/repo::workflow": [seconds, ...] }`, last 20 per key, successful runs
only. Written via temp file + rename so an interrupted write cannot truncate it.

No database. No migrations. A corrupt file is reported and replaced with an
empty one.

## Module Structure

```
src/
  main.rs              # CLI parsing, subsystem/console setup, wiring
  config.rs            # Parse, validate, hot-reload; Reloader shared with the tray
  config_template.toml # Written verbatim on first run (include_str!)
  filter.rs            # exclude patterns: owner/repo globbing
  github.rs            # REST client, conditional-request cache, API types
  poller.rs            # Supervisor + one AccountPoller per account
  history.rs           # Duration history and progress estimation
  model.rs             # Snapshot types shared across the thread boundary
  state.rs             # UI card list, snapshot diffing, notices, panel
  check.rs             # --check dry run
  demo.rs              # --demo scripted offline replay
  autostart.rs         # HKCU Run key
  console.rs           # Console attach for a windows-subsystem binary
  logging.rs           # tracing -> rolling daily file
  paths.rs             # %APPDATA% locations
  ui/
    mod.rs             # eframe App: logic/ui split, window lifecycle
    card.rs            # Card painting (run, message, watched panel)
    theme.rs           # Colours and the fixed-height metrics
    tray.rs            # Tray icon, generated bitmap, menu
    win.rs             # Win32 styles, placement, DPI
```

## Concurrency Model

```
main thread (winit/egui)          backend thread (tokio)
--------------------------        ------------------------------------
MonitorApp::logic  <--------.     supervisor
  ingest Snapshot           |       |- reconcile tasks against config
  size/place/show window    |       |- AccountPoller (personal)
  handle tray commands      |       |- AccountPoller (work)
MonitorApp::ui              |       `- ...
  draw cards                |
                            `----- watch::Sender<Arc<Snapshot>>
                                   + Context::request_repaint()
```

- The UI never blocks on the network. The backend never touches egui state.
- Config changes flow the other way through a `watch::Sender<Arc<Config>>` owned
  by a `config::Reloader`, shared between the file watcher and the tray menu so
  a manual reload and a save-triggered reload take the identical path.

## Polling Cadence

| Mode | Cadence | What |
| --- | --- | --- |
| Idle | `poll_idle_seconds` (60) | Per repo: `?status=queued` and `?status=in_progress`, both conditional |
| Active | `poll_active_seconds` (5) | Per *run*: the run itself plus its jobs. Other repos stay idle |
| Discovery | 1 hour | `GET /user/repos?sort=pushed`, filtered by `discover_days`, then `exclude`, then capped |
| Low rate limit | 5 minutes | Triggered below 100 remaining |
| Backoff | 5s doubling to 120s | Transient failures only |

Exclusions are applied **before** the cap so ignoring a noisy org frees slots
rather than wasting them.

## Hard Constraints

- Re-assert `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` every tick; never assume they
  survived.
- Show with `SW_SHOWNOACTIVATE`, move with `SWP_NOACTIVATE`. Never
  `ViewportCommand::Focus`.
- Card heights stay compile-time constants; no content-dependent sizing.
- No `unwrap()`/`expect()` on fallible paths in the steady-state loop.
- Never `panic = "abort"`.
- Tokens never reach a log line, an error message, or the UI.
- `deny_unknown_fields` on config structs, so a typo is an error rather than a
  silently ignored setting.
