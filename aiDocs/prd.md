# actions-monitor — Product Requirements

Version: 1.0
Status: Delivered (v0.1.0)
Last updated: 2026-08-21
Platform: Windows 10/11, x86_64
Scope: v1

## Product Overview

### What it is

A background Windows utility that surfaces in-flight GitHub Actions runs as a
small, unobtrusive popup in the corner of the screen, and gets out of the way
the moment there is nothing to show.

### What it is not

Not a CI dashboard. Not a log viewer. Not a GitHub client. It shows *that*
something is running and roughly how far along it is — never *why* it failed.
When you need that, you click the card and land on github.com, which is already
good at it.

### The problem

Waiting on CI is a context-switching tax. The options today are all bad:

- Leaving a browser tab open on the Actions page and alt-tabbing to it.
- Waiting for email or Slack notifications, which arrive only when the run has
  already finished.
- Polling manually with `gh run watch`, which pins a terminal.

None of these tell you *at a glance* whether the thing you are waiting on is
still going, and none of them handle more than one GitHub identity at once.
Anyone with a personal account plus a work account has to repeat the whole
routine per account.

### Positioning

The product competes with "a browser tab you keep checking". It wins by being
ambient: zero interaction in the common case, and zero presence when idle.

### Anti-features — deliberately excluded

| Excluded | Why |
| --- | --- |
| Write access of any kind | Read-only means a leaked token is far less dangerous, and a fine-grained token needs only Actions:Read |
| Re-run / cancel controls | Turns a glance into a console, and demands write scopes |
| Log or job output viewing | github.com already does this well |
| Notifications, sounds, toasts | The product's value is *not* interrupting |
| Settings GUI | The TOML file hot-reloads; a GUI would be more surface for less value |
| Telemetry | Nothing leaves the machine except requests to api.github.com |

## Target Users

### Primary

Developers who work across **more than one GitHub identity** — typically a
personal account plus an employer's, often plus one or more client or side-project
organisations — and who are waiting on CI several times a day.

### Secondary

Single-account developers who simply want ambient build status without a
dedicated screen or browser tab.

### Not for

- Teams wanting a shared/wall-mounted build radiator. This is single-user and
  bound to one desktop session.
- Anyone needing to act on runs (re-run, cancel, approve). Read-only by design.
- Non-Windows users. The window behaviour is Win32-specific and not portable.

### Personas

**Maya — contractor, four GitHub identities.** Personal account, two client
orgs, one employer. Her problem is not any single build; it is that she cannot
hold four Actions tabs in her head. She wants one place that shows whatever is
running, whoever owns it, and needs it to cost her nothing when nothing is.

**Dan — backend developer, one employer.** Pushes, then waits three to eight
minutes. He wants to know whether to start something else or wait it out, which
means he needs *elapsed vs typical*, not just "running".

<!-- TODO: Both personas are inferred from the build conversation, not from user
     research. Validate before treating them as requirements. -->

## Core Features

### 1. The popup

**Purpose.** Show every active run at a glance without becoming a window you
have to manage.

**Flow.**

```
nothing running        -> no window at all, tray icon grey
run detected           -> window fades in, bottom-left, above the taskbar
more runs start        -> stack grows upward; bottom-left corner never moves
run finishes           -> card shows result for linger_seconds (default 8)
last card expires      -> window disappears entirely
```

**Rules.**

- Never activates, never takes foreground, no taskbar button, no Alt-Tab entry.
- Anchored to the primary monitor's *work area*, so it sits above the taskbar
  wherever the taskbar is.
- A stack too tall for the screen pins to the top of the work area rather than
  running off it.
- Clicking a card opens that run on github.com. Clicking its X dismisses that
  run for as long as it exists, even while still running.

### 2. Multi-account polling

**Purpose.** Treat "an account" as *one token plus the repos it watches*, so any
number of identities work the same way.

**Rules.**

- One task per account, its own token, its own rate-limit budget, its own
  conditional-request cache. Accounts cannot interfere with each other.
- One account failing (bad token, unreadable repos) never affects another.
- The same repo watched by two accounts still produces exactly one card.

### 3. Estimated progress

**Purpose.** Turn "running" into "roughly two thirds done", which is what
actually informs the wait-or-switch decision.

**Rules.**

- Durations of *successful* runs are recorded per `owner/repo::workflow`, last
  20 kept, in `%APPDATA%\actions-monitor\history.json`.
- Progress is `elapsed / median`, capped at 95% so an overrunning run looks
  nearly-done but never done.
- With no history the bar pulses rather than inventing a number.
- Only successful runs are recorded: failures usually abort early and would drag
  the median down.

### 4. Tray icon

**Purpose.** Persistent status when the popup is (correctly) invisible.

**Rules.**

- Colour is live state: grey idle, blue running, green/red/amber for the last
  result, red while any account has stopped polling.
- Status is read from the backend's live view, not the card list — a card times
  out after 30 seconds, but a broken account has not.
- Left-click shows the watched-repo panel. Right-click opens the menu.

### 5. Configuration and diagnostics

**Purpose.** Make misconfiguration obvious and fast to fix, since that is where
all the real friction is.

**Rules.**

- Config is validated at load; errors name the offending account.
- A mid-edit invalid file is ignored with a warning; the running app keeps the
  last good config.
- `--check` is a read-only dry run: token identity, scopes, exactly which repos
  auto-discovery resolves to, what each `exclude` pattern removed, and whether
  Actions can be read on each one.

## Technical Requirements

| Layer | Decision |
| --- | --- |
| Language | Rust 2024 edition |
| Async | tokio, multi-thread runtime on a dedicated thread |
| HTTP | reqwest with rustls (no OpenSSL on Windows) |
| UI | eframe / egui, single frameless transparent viewport |
| Window control | Win32 directly, via the `windows` crate |
| Storage | TOML config, JSON history, under `%APPDATA%` |
| Distribution | A single self-contained `.exe` |

### Privacy and security

- Read-only against the GitHub API; no other host is contacted.
- Tokens live in the config file or an environment variable and are never
  logged, printed, or included in an error message.
- No telemetry, no crash reporting, no account system.

### Performance

| Target | Requirement |
| --- | --- |
| Idle rate-limit cost | Effectively zero — conditional requests return 304, which GitHub does not count |
| Idle CPU | Negligible; the process sleeps between polls and paints nothing while hidden |
| Time to notice a new run | <= `poll_idle_seconds` (default 60s) |
| Job/step refresh while active | `poll_active_seconds` (default 5s) |
| Popup frame rate | 20fps while a run is live, 5fps while only lingering |

## Design Principles

1. **Ambient, not attentional.** If a change would make the app ask for
   attention, it is wrong. This is why there are no notifications and why focus
   stealing is treated as a bug rather than a trade-off.
2. **Invisible when idle.** No window, no taskbar entry. The tray icon is the
   only permitted idle presence, and it is opt-out.
3. **Cheap by default.** Conditional requests everywhere. If a poll costs rate
   limit when nothing changed, the design is wrong.
4. **Fail small.** One bad token degrades one account. Transient failures back
   off. The app does not exit.
5. **Never leak a secret.** Including into logs and error messages, which is
   where it actually happens.

## Success Metrics

| Metric | Target | Why |
| --- | --- | --- |
| Rate limit consumed while idle | ~0 / hour | The whole conditional-request design exists for this |
| Unintended focus steals | 0 | A single one makes the app unusable |
| Crashes / unplanned exits | 0 over weeks | It is an autostart background app |
| Time from run start to card | < 60s | Beyond this it stops being useful |

| Qualitative | How measured | Why |
| --- | --- | --- |
| "I forget it is running" | Self-report | The product working as intended |
| Configuration succeeds first try | Whether `--check` is needed to diagnose | Setup is the main friction point |

| Failure indicator | Detection | Meaning |
| --- | --- | --- |
| Rate-limit warnings in the log | `remaining < 100` warning | Too many repos, or the cache is not working |
| Repos silently unwatched | `--check` shows "over the cap" | `discover_max_repos` is too low |
| Cards duplicated | Two cards, same run | Account de-duplication regressed |

## Competitive Landscape

| Alternative | Category | Why users leave it | Contrast |
| --- | --- | --- | --- |
| Browser tab on the Actions page | Manual | Requires alt-tabbing and remembering to look | Ambient; no interaction needed |
| Email / Slack notifications | Push | Only fires when the run is already over | Live progress while it matters |
| `gh run watch` | CLI | Pins a terminal, one run at a time, one account | Multi-account, no terminal |
| GitHub mobile app | Push | Wrong device, still notification-based | On the machine you are working on |

<!-- TODO: No structured competitive research was done; this table is reasoning
     from the alternatives discussed, not a survey. -->

## Out of Scope for v1

| Feature | Rationale |
| --- | --- |
| macOS / Linux | Window behaviour is Win32-specific; a port is a rewrite of the UI layer |
| Re-run / cancel | Requires write scopes, which undermines the security posture |
| Log viewing | github.com is one click away and better at it |
| Shared/team view | Single-user, single-session by design |
| Settings GUI | Hot-reloading TOML is enough |
| Notifications | Directly contradicts the core principle |

## Open Questions

1. Is `discover_max_repos = 25` the right default? Real usage hit 9 of 10 on one
   account almost immediately, suggesting the default may be low for people with
   many active repos.
2. Should the tray icon keep showing the *last result* after the linger expires,
   rather than reverting to idle grey? Currently it reverts.
3. Should `exclude` also be settable globally rather than per account?
4. Is a 45-second auto-close right for the watched-repos panel, or should it stay
   until dismissed?
