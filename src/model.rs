//! Types shared between the polling backend and the UI thread.
//!
//! The backend produces immutable [`Snapshot`]s; the UI diffs each new snapshot
//! against the cards it is already showing (see `crate::state`).

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

/// Identifies one workflow run. Run ids are unique per GitHub instance, but we
/// key by account as well so two configured accounts can never collide.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunKey {
    pub account: String,
    pub run_id: u64,
}

impl RunKey {
    pub fn new(account: impl Into<String>, run_id: u64) -> Self {
        Self { account: account.into(), run_id }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Queued,
    InProgress,
    Completed,
}

impl RunStatus {
    /// GitHub reports several pre-run states (`queued`, `waiting`, `pending`,
    /// `requested`); we fold them all into [`RunStatus::Queued`].
    pub fn parse(raw: &str) -> Self {
        match raw {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            _ => Self::Queued,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::InProgress)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    Neutral,
    Other,
}

impl Conclusion {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "cancelled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            "timed_out" => Self::TimedOut,
            "action_required" => Self::ActionRequired,
            "neutral" => Self::Neutral,
            _ => Self::Other,
        }
    }

    /// A glyph for the finished card. Restricted to code points egui's bundled
    /// emoji font actually carries, so nothing renders as tofu.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Success => "\u{2705}",                          // white heavy check mark
            Self::Failure | Self::TimedOut => "\u{274c}",         // cross mark
            Self::Cancelled | Self::Skipped | Self::ActionRequired => "\u{26a0}", // warning sign
            Self::Neutral | Self::Other => "\u{2022}",            // bullet
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "Succeeded",
            Self::Failure => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Skipped => "Skipped",
            Self::TimedOut => "Timed out",
            Self::ActionRequired => "Action required",
            Self::Neutral => "Neutral",
            Self::Other => "Finished",
        }
    }
}

/// The job/step a run is currently executing, used for the `job > step (3/7)` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobProgress {
    pub job_name: String,
    pub step_name: Option<String>,
    /// 1-based index of the running step within the job.
    pub step_index: Option<u32>,
    pub step_total: Option<u32>,
}

/// Everything the UI needs to draw one card. Cheap to clone (a handful of small
/// `String`s), and produced fresh by the poller on every update.
#[derive(Clone, Debug, PartialEq)]
pub struct RunView {
    pub key: RunKey,
    /// `owner/repo`.
    pub repo: String,
    pub workflow: String,
    pub run_number: u64,
    pub branch: String,
    pub head_sha: String,
    /// First line of the head commit message, already trimmed.
    pub commit_subject: String,
    pub status: RunStatus,
    pub conclusion: Option<Conclusion>,
    pub started_at: DateTime<Utc>,
    pub html_url: String,
    pub current: Option<JobProgress>,
    /// Median duration of previous successful runs of this workflow, if known.
    pub estimate: Option<Duration>,
    /// When *we* observed the run reach `completed`. Drives the linger timer, so
    /// a run that finished while the app was closed still gets its full moment.
    pub finished_at: Option<Instant>,
}

impl RunView {
    pub fn short_sha(&self) -> &str {
        let n = self.head_sha.len().min(7);
        &self.head_sha[..n]
    }

    /// Wall-clock duration the run has been going, frozen once it completes.
    pub fn elapsed(&self, now: DateTime<Utc>, now_instant: Instant) -> Duration {
        match self.finished_at {
            Some(finished) => {
                let since_finish = now_instant.saturating_duration_since(finished);
                let raw = (now - self.started_at).to_std().unwrap_or_default();
                raw.saturating_sub(since_finish)
            }
            None => (now - self.started_at).to_std().unwrap_or_default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// The account's token was rejected (401). Polling for it has stopped.
    Auth,
    /// Something else worth surfacing once (e.g. every watched repo 404s).
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountIssue {
    pub account: String,
    pub kind: IssueKind,
    pub message: String,
    pub raised_at: Instant,
}

/// A repository the poller is watching, and when it last heard from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchedRepo {
    pub account: String,
    /// `owner/repo`.
    pub repo: String,
    /// When this repo was last polled successfully. `None` before the first
    /// sweep completes.
    pub last_checked: Option<Instant>,
    /// False once the repo has answered 404/403 and been skipped.
    pub readable: bool,
}

/// One immutable view of the world, published to the UI over a watch channel.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub runs: Vec<RunView>,
    pub issues: Vec<AccountIssue>,
    /// Every repo currently being polled, across all accounts.
    pub watched: Vec<WatchedRepo>,
    /// How long a finished card stays on screen, mirrored from the config so the
    /// UI picks up hot-reloaded values without its own config handle.
    pub linger: Duration,
}

impl Snapshot {
    pub fn empty(linger: Duration) -> Self {
        Self {
            runs: Vec::new(),
            issues: Vec::new(),
            watched: Vec::new(),
            linger,
        }
    }
}

/// Coarse "how long ago", for the watched-repo list: `just now`, `2m ago`.
pub fn format_ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=9 => "just now".to_owned(),
        10..=89 => format!("{secs}s ago"),
        // Switch to hours at 59.5 minutes, so rounding can never print
        // "60m ago" instead of "1h ago".
        90..=3569 => format!("{}m ago", (secs + 30) / 60),
        _ => format!("{}h ago", (secs + 1800) / 3600),
    }
}

/// Human-readable elapsed time: `45s`, `2m 14s`, `1h 03m`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Truncate on a char boundary, appending an ellipsis when anything was cut.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_format_readably() {
        assert_eq!(format_duration(Duration::from_secs(9)), "9s");
        assert_eq!(format_duration(Duration::from_secs(74)), "1m 14s");
        assert_eq!(format_duration(Duration::from_secs(3780)), "1h 03m");
    }

    #[test]
    fn ago_is_coarse_and_readable() {
        assert_eq!(format_ago(Duration::from_secs(3)), "just now");
        assert_eq!(format_ago(Duration::from_secs(45)), "45s ago");
        assert_eq!(format_ago(Duration::from_secs(120)), "2m ago");
        assert_eq!(format_ago(Duration::from_secs(3600)), "1h ago");
        assert_eq!(format_ago(Duration::from_secs(7200)), "2h ago");
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_chars("short", 10), "short");
        assert_eq!(truncate_chars("abcdefghij", 5), "abcd\u{2026}");
        // Multi-byte input must not panic or split a code point.
        assert_eq!(truncate_chars("héllo wörld", 6), "héllo\u{2026}");
    }

    #[test]
    fn statuses_fold_pre_run_states_into_queued() {
        assert_eq!(RunStatus::parse("waiting"), RunStatus::Queued);
        assert_eq!(RunStatus::parse("requested"), RunStatus::Queued);
        assert_eq!(RunStatus::parse("in_progress"), RunStatus::InProgress);
        assert_eq!(RunStatus::parse("completed"), RunStatus::Completed);
        assert!(RunStatus::parse("queued").is_active());
        assert!(!RunStatus::parse("completed").is_active());
    }
}
