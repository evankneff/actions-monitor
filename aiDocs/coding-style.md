# Coding Style

Rules specific to this repo. Generic Rust best practice is assumed.

- **No `unwrap()` or `expect()` on fallible paths in the steady-state loop.** Use
  `anyhow` at the application edge, `thiserror` for typed library errors, and
  `let ... else` / `if let` for the rest. Tests may use `expect("message")`.
- **All output goes through `tracing`.** No `println!` outside the CLI command
  handlers in `main.rs`, which print for the user rather than log.
- **Never let a token reach a log line, an error message, or the UI.** Errors
  name the account or the environment variable. There is a test asserting this;
  keep it passing.
- **Card layout uses fixed heights.** Every row is allocated at an exact height
  with `item_spacing` zeroed, and totals live in `ui::theme` as constants. The
  window is sized before any egui pass, so content-dependent height is a bug.
- **Comments explain why, not what.** Prefer one sentence on a non-obvious
  decision over a paragraph restating the code. Win32 and egui quirks deserve a
  comment; a `for` loop does not.
- **Tests are named as sentences** describing the behaviour being protected
  (`dismissed_runs_stay_dismissed_while_still_running`), and assert on behaviour
  rather than implementation detail.
- **`cargo clippy --all-targets` must be clean.** No `#[allow]` without a
  comment justifying it.
- **Check `aiDocs/architecture.md` before adding a dependency.** It records what
  was deliberately rejected and why.
- **Do not add abstraction before the second use.** No traits with one
  implementor, no generic helpers with one caller, no "we might need this later"
  stubs.
