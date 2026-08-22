# actions-monitor — High-Level Project Plan

Created: 2026-08-21
Status: Active — Phases 0-3 complete, Phase 4 open

## Engineering Philosophy

This is a single-purpose desktop utility, not a platform. The failure mode to
guard against is not under-engineering, it is building scaffolding for a product
that will never exist.

Resist:

- Abstractions before the second use. No traits with one implementor, no generic
  helpers with one caller.
- Layers for their own sake. The poller talks to the API and publishes a
  snapshot; there is no repository, service and mapper in between.
- "We might need this later" stubs and dead code.
- Dependencies that do slightly more than needed. Six CLI flags do not justify a
  argument-parsing framework; four endpoints do not justify an API client crate.

Prefer instead:

- Verifying Windows behaviour by running the app and inspecting the real window
  from outside the process. Window styles, focus and tray input do not behave the
  way the documentation implies, and three real bugs in this project were only
  visible that way.
- Tests on logic that is easy to get subtly wrong — snapshot diffing, progress
  estimation, glob matching, config validation — and manual verification for
  anything involving the compositor.

## Phase 0 — Foundation (complete)

- [x] Cargo project, Rust 2024, `x86_64-pc-windows-msvc`
- [x] Dependency selection, versions resolved against crates.io
- [x] Verify rustls/aws-lc-rs builds on Windows without OpenSSL
- [x] `%APPDATA%` paths, rolling-file logging via `tracing`
- [x] Windows-subsystem binary with console attach for CLI use

## Phase 1 — MVP: window behaviour first (complete)

Deliberately built and validated *before* any real API work, using `--demo`.

- [x] Frameless, transparent, always-on-top viewport
- [x] Bottom-left placement in the primary monitor work area, in physical pixels
- [x] Stack grows upward with a fixed bottom-left corner
- [x] No focus steal, no taskbar entry, verified externally
- [x] Appear / linger / disappear lifecycle
- [x] `--demo` scripted replay covering every run state
- [x] Card rendering with fixed-height rows

## Phase 2 — Real data (complete)

- [x] GitHub client with conditional requests and an ETag cache
- [x] One poller task per account, supervised and reconciled against config
- [x] Idle / active cadences, discovery, rate-limit backoff
- [x] Snapshot publishing over `watch`; UI-side diffing by run id
- [x] Duration history and progress estimation
- [x] Per-account and per-repo error isolation; 401 handling
- [x] TOML config with validation and hot-reload

## Phase 3 — Polish and stability (complete)

- [x] Tray icon with live status colour and a generated bitmap
- [x] Tray menu: watched repos, reload, open config, open logs, autostart, quit
- [x] Watched-repos panel with last-checked times
- [x] `--check` dry run: identity, scopes, resolved repos, per-repo access
- [x] `exclude` patterns with glob matching
- [x] Run de-duplication across accounts
- [x] Autostart install/uninstall/status
- [x] Public-repo preparation: MIT licence, .gitignore, metadata, scrubbed fixtures
- [x] 91 unit tests, clippy clean

## Phase 4 — Future (deferred)

Nothing here gets built without evidence from real use.

Candidates, roughly in order of how likely they are to matter:

- Revisit `discover_max_repos` default — real usage hit 9 of 10 immediately.
- Tray icon retaining the last result rather than reverting to idle grey.
- Global `exclude` in addition to per-account.
- Configurable window corner, for people whose taskbar or workflow makes
  bottom-left wrong.
- Watched-repo panel behaviour: persistent until dismissed rather than a 45s
  auto-close.
- Per-account colour accent, if many accounts turns out to be visually confusing.

Explicitly not planned: write operations, log viewing, notifications. Cross-platform
support was in this category too, until Evan asked for a macOS port on 2026-08-21 (see
Phase 5) — see `aiDocs/prd.md` for the original Windows-only rationale, now superseded
for the window/platform layer specifically.

## Phase 5 — macOS port (in progress)

Reference: [2026-08-21-macos-port.md](2026-08-21-macos-port.md),
[2026-08-21-roadmap-macos-port.md](2026-08-21-roadmap-macos-port.md)

- [x] Compile on macOS (`windows`/`winreg` moved to `cfg(windows)` deps; `console.rs`,
      `autostart.rs`, `ui/win.rs` cfg-gated)
- [x] `ui/mac.rs`: non-activating popup via the `NSPanel` class swap
- [x] `cargo test` — 91/91, matching the documented Windows count exactly (4
      Windows-only `win.rs` tests replaced one-for-one by 4 macOS `mac.rs` tests)
- [x] `cargo clippy --all-targets` clean
- [x] Launch evidence: class swap, KVO self-check and style-mask/level/policy all
      logged and correct on a real run (`--demo`)
- [ ] Visual confirm (click-without-activation, keystrokes) — needs Evan, see
      `MACOS_RUNBOOK.md`
- [ ] Graceful-exit path (`Popup::restore`, no `NSAutoreleasePool` double-drain) — needs
      the tray's Quit item, i.e. a mouse; not reachable headlessly
- [ ] macOS-native data directory (`~/Library/Application Support/actions-monitor`
      instead of `%APPDATA%`) — found blocking even `--demo` during this work, parked as
      a separate concern from window behaviour; see the plan doc's "Parked" section
- [ ] `autostart.rs` macOS arm (`LaunchAgent` plist) — parked, currently returns a clear
      "not implemented" error instead of pretending

## Phase Plan & Roadmap Docs

| Phase | Plan | Roadmap | Location |
| --- | --- | --- | --- |
| 0-3 | — | — | Built in a single session; this document is the record |
| 5 | [2026-08-21-macos-port.md](2026-08-21-macos-port.md) | [2026-08-21-roadmap-macos-port.md](2026-08-21-roadmap-macos-port.md) | `ai/roadmaps/` |

<!-- TODO: Phases 0-3 predate the plan/roadmap pair convention. Future phases
     get a YYYY-MM-DD-phase-N-name.md plus YYYY-MM-DD-roadmap-phase-N-name.md
     pair in ai/roadmaps/, moved to complete/ when done. -->
