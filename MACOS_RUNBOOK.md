# macOS runbook — actions-monitor

Two minutes, screen unlocked. Confirms the thing no automated check can: that the popup
does not steal focus or the menu bar. Everything else (class swap, style mask, KVO
self-check, activation policy) was already confirmed at runtime by `cargo test` + a
`--demo` launch — see `ai/roadmaps/2026-08-21-macos-port.md`.

```sh
cd ~/dev/actions-monitor
PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build
```

No `APPDATA` workaround is needed. As of `9191ed2`, `paths.rs` resolves the data
directory natively on macOS to `~/Library/Application Support/actions-monitor`, and the
binary writes a starter `config.toml` there on first run. `--demo` replays a scripted
run and never touches the network, so no GitHub token is required for this check.

```sh
open TextEdit  # or any app — this is what focus-stealing would interrupt
```

1. Click into a TextEdit document and start typing.
2. Without clicking away, run (from a terminal, backgrounded or in another Space):
   ```sh
   RUST_LOG=debug ./target/debug/actions-monitor --demo --console
   ```
3. Within a few seconds a small card appears in the bottom-left corner (the demo script
   replays a scripted run).

| # | Check | Pass |
|---|-------|------|
| 1 | Keep typing while the card appears | characters keep landing in TextEdit |
| 2 | Menu bar | still says "TextEdit", not "actions-monitor" |
| 3 | **Click the card** (it opens a browser — that's expected and fine) | after the click, menu bar still says "TextEdit" once you switch back |
| 4 | Cmd-Tab | **no** entry for actions-monitor |
| 5 | Dock | no icon, no bounce, at any point |
| 6 | Position | bottom-left corner, correctly inset, growing upward as more cards appear |
| 7 | Tray icon | present, opens its menu, "Quit" ends the process cleanly |

Then scan the log for these three lines, in order:

```
popup window class swapped from=NSKVONotifying_WinitWindow to=ActionsMonitorNonactivatingPanel
KVO appearance self-check passed (dark -> system, no abort)
popup window configured: no focus steal, no taskbar entry (class=NSKVONotifying_ActionsMonitorNonactivatingPanel mask=0x0084 level=25 key=false app_active=false policy=NSApplicationActivationPolicy(1) frontmost="TextEdit")
```

`class=` must contain `ActionsMonitorNonactivatingPanel`. `frontmost=` should read
`"TextEdit"` (or whatever app you were using) on every subsequent line, never
`"actions-monitor"`.

**Quit from the tray's Quit item, not Ctrl-C.** This is also the one check this session
could not run at all: quitting via the tray is the only path that calls
`eframe::App::on_exit`, which restores the window's original class before winit tears it
down. Watch the log for:

```
popup window class restored to NSKVONotifying_WinitWindow
```

and confirm there is **no** `NSAutoreleasePool drain: This pool has already been
drained` line after it. If that line does appear, the process still exits 0 - it is a
console message, not a crash - but it means something about the restore ordering
regressed; see `FINDINGS.md` §4.7 (not in this repo) or `ui/mac.rs::Popup::restore`'s doc
comment.

## If something looks wrong

| Symptom | Look at |
|---|---|
| binary won't start, `APPDATA environment variable is not set` | **stale symptom — fixed in `9191ed2`.** If you genuinely see this, you are running a binary built before that commit; rebuild. |
| `No such file or directory` for `~/Library/Application Support/actions-monitor/config.toml` | expected on a truly first run — the binary writes a starter config there and exits; run it again |
| card never appears | log for `could not resolve the native window handle` |
| card appears but steals focus on click | log's `class=` — if it doesn't say `ActionsMonitorNonactivatingPanel`, the swap didn't happen |
| process aborts on exit | `ui/mac.rs`'s KVO doc comments (`suspend_winit_kvo`/`resume_winit_kvo`) |
| process aborts on a light/dark switch | same — check the re-registration options are `New \| Old`, not `empty()` |
| `NSAutoreleasePool` line at quit | `Popup::restore` — confirm `on_exit` actually ran (i.e. you quit from the tray, not `kill`) |
