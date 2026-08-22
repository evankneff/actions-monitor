# Roadmap — macOS Port

Created 2026-08-21 · Status: In progress — visual confirm and two parked items remain
Plan: [2026-08-21-macos-port.md](2026-08-21-macos-port.md)

## Milestone 1 — Compile

- [x] `windows`/`winreg` moved to `[target.'cfg(windows)'.dependencies]`
- [x] `console.rs`: real impl behind `#[cfg(windows)]`, no-op `attach`/`detach` on macOS
- [x] `autostart.rs`: real impl behind `#[cfg(windows)]`, placeholder error on macOS
- [x] `ui/win.rs` and its use in `ui/mod.rs` behind `#[cfg(windows)]`
- [x] `cargo check` clean on macOS (own commit, before any macOS window code)

## Milestone 2 — The popup

- [x] `ui/mac.rs`: `NonactivatingPanel`, `Popup::adopt`/`enforce`/`restore`, KVO
      suspend/resume dance, `poke_appearance` self-check, `bottom_left`
- [x] `main.rs`: `event_loop_builder` hook (`Accessory`,
      `with_activate_ignoring_other_apps(false)`, `with_default_menu(false)`)
- [x] `ui/mod.rs`: `popup: Option<mac::Popup>` field, macOS arms of
      `reposition`/`enforce_styles`/`update_visibility`, `eframe::App::on_exit`
- [x] `cargo test` — 91/91 (unchanged from the documented Windows count: 4 `win.rs`
      tests dropped on this target, 4 new `mac.rs` tests added)
- [x] `cargo clippy --all-targets` clean

## Milestone 3 — Evidence

- [x] `--demo` launch logs the class swap
      (`NSKVONotifying_WinitWindow -> ActionsMonitorNonactivatingPanel`), the KVO
      self-check passing, and `class=NSKVONotifying_ActionsMonitorNonactivatingPanel
      mask=0x0084 level=25 … policy=Accessory`
- [ ] Visual confirm against an unlocked screen — `MACOS_RUNBOOK.md`. **Not run this
      session; needs Evan.**
- [ ] Graceful-exit path (`Popup::restore`, no autorelease-pool message) — needs the
      tray's Quit item, i.e. a mouse. **Not reachable headlessly.**

## Parked, not blocking

- [ ] macOS-native data directory (`paths.rs` hardcodes `%APPDATA%`, blocks even
      `--demo` without a manual env var override)
- [ ] `autostart.rs` macOS `LaunchAgent` implementation
