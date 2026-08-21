# actions-monitor — MVP

Version: 1.0
Status: Complete — shipped as v0.1.0
Last updated: 2026-08-21
Goal: prove that ambient, non-interrupting build status is more useful than a browser tab.

## What This MVP Is

The minimum that answers one question: **does a popup that never takes focus and
disappears on its own actually feel better than alt-tabbing to the Actions page?**

Everything else — estimates, the tray icon, `--check`, exclusion patterns — was
built only after the window behaviour proved out. That ordering was deliberate:
the Windows-specific behaviour is the fiddly part and the part most likely to
kill the idea, so it was validated first with fake data.

## The Core Loop

```
idle: no window, no taskbar entry
  |
  |  a workflow run starts
  v
poller notices within poll_idle_seconds
  |
  v
popup fades in bottom-left, above the taskbar
  repo / workflow / branch / commit
  current job and step, elapsed ticking
  progress bar
  |
  |  run finishes
  v
card shows the result for linger_seconds
  |
  v
window disappears entirely -> back to idle
```

Repeatable indefinitely, and the whole loop requires zero interaction.

## Screens In Scope

There is one window and no navigation. "Screens" are card types.

### Run card

**Purpose.** Everything you need to decide wait-or-switch, in one glance.

**Visual.** ~340pt wide, dark, rounded, with a coloured stripe down the left
edge. Five fixed-height rows: repo (bold) with a dismiss X; workflow and run
number; branch, short SHA and commit subject; current job/step with elapsed time
right-aligned; a progress bar.

**Behavior.** Fades in over 220ms. Elapsed ticks at 20fps while live. Clicking
opens the run on github.com; clicking the X dismisses that run permanently.
Hovering lightens the background.

**Notes.** Stripe and bar are colour-coded — blue running, grey queued, then
green/red/amber. Every text row truncates rather than wrapping, because the
window is sized from constants before anything is drawn.

### Message card (account issue / notice)

**Purpose.** Say which account stopped polling and why, without a dialog.

**Visual.** Same shell, three rows, red stripe for errors and blue for notices.

**Behavior.** Any click dismisses. Issues auto-expire after 30 seconds, notices
after 6.

### Watched-repos panel

**Purpose.** Show what is actually being polled — invisible otherwise when
auto-discovery decides the list.

**Visual.** One row per repo, grouped by account, with last-checked time
right-aligned. Unreadable repos in red.

**Behavior.** Opened by left-clicking the tray icon or the menu item. Toggles
closed; auto-closes after 45 seconds.

## Data Persistence

| Stored | Where | Notes |
| --- | --- | --- |
| Accounts, tokens, timings | `%APPDATA%\actions-monitor\config.toml` | Hot-reloads on save |
| Run durations | `%APPDATA%\actions-monitor\history.json` | Successful runs, last 20 per workflow |
| Logs | `%APPDATA%\actions-monitor\logs\` | Rolling daily |

Not stored: run details, job logs, anything about a run once it has left the
screen. Dismissals are in-memory and intentionally forgotten when the run
disappears.

## Out of Scope for MVP

| Feature | When |
| --- | --- |
| Tray icon | Delivered post-MVP, in the same cycle |
| `--check` diagnostics | Delivered post-MVP, prompted by real setup friction |
| `exclude` patterns | Delivered post-MVP, once auto-discovery proved too blunt |
| Re-run / cancel | Never — requires write scopes |
| Log viewing | Never — github.com is one click away |
| Cross-platform | Not planned |

## Demo Script

Under three minutes, no GitHub account required:

1. `actions-monitor --demo`. Nothing appears — the app is running and idle.
2. After ~2s a card fades in bottom-left. Point out: no taskbar button, and the
   window did not steal focus — keep typing in the editor to show it.
3. A second and third card appear; the stack grows *upward* while the bottom-left
   corner stays put.
4. Point out the differences: one card has a filled progress bar (learned
   history), another pulses (no history yet).
5. Cards finish — green, red, amber — linger, then the window vanishes entirely.
6. Left-click the tray icon to show the watched-repo list; right-click for the
   menu.
7. The cycle restarts every 52 seconds.

## Definition of Done

- [x] Window appears bottom-left of the primary work area with a 16px margin
- [x] Stack grows upward; the bottom-left corner never moves
- [x] Window never becomes foreground, verified from outside the process
- [x] No taskbar button and no Alt-Tab entry
- [x] Window disappears entirely when the last card expires
- [x] Cards update in place rather than being recreated
- [x] Clicking a card opens the run; the X dismisses only that run
- [x] Progress bar estimates from history, pulses without it, caps at 95%
- [x] Two or more accounts poll independently; one failing does not affect others
- [x] 401 shows a card naming the account and stops only that account
- [x] Config hot-reloads; an invalid edit is ignored with the last good config kept
- [x] Idle polling costs effectively no rate limit (304s)
- [x] Single self-contained binary, no console at login
- [x] `cargo test` and `cargo clippy --all-targets` clean
