//! A scripted, offline replay of the window's whole lifecycle.
//!
//! `actions-monitor --demo` runs this instead of touching GitHub: cards appear,
//! progress, finish with each of the three outcomes, linger, and the window
//! disappears - then the cycle repeats. It is the quickest way to check the
//! Windows-specific behaviour (placement, always-on-top, no focus steal, the
//! appear/disappear lifecycle) without waiting for a real workflow to run.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::watch;

use crate::model::{
    Conclusion, JobProgress, RunKey, RunStatus, RunView, Snapshot, WatchedRepo,
};
use crate::poller::Notify;

/// Length of one full cycle.
const CYCLE: f64 = 52.0;
/// How often the fake backend publishes.
const TICK: Duration = Duration::from_millis(250);

/// Repos the fake backend claims to be watching, so the tray's "Show watched
/// repos" panel has something realistic in it during a demo.
const WATCHED: &[(&str, &str, u64)] = &[
    ("personal", "me/actions-monitor", 12),
    ("personal", "me/dotfiles", 47),
    ("personal", "me/notes", 200),
    ("work", "work-org/infra", 8),
    ("work", "work-org/app", 33),
];

struct Script {
    run_id: u64,
    repo: &'static str,
    workflow: &'static str,
    branch: &'static str,
    sha: &'static str,
    subject: &'static str,
    /// Seconds into the cycle at which the run appears.
    start: f64,
    /// Seconds into the cycle at which it completes.
    finish: f64,
    conclusion: Conclusion,
    /// Median from "history"; `None` exercises the indeterminate bar.
    estimate: Option<u64>,
    steps: &'static [(&'static str, &'static str)],
}

const SCRIPT: &[Script] = &[
    Script {
        run_id: 1001,
        repo: "me/actions-monitor",
        workflow: "CI",
        branch: "main",
        sha: "a1b2c3d4e5f60718",
        subject: "fix: do not steal focus when the popup appears",
        start: 2.0,
        finish: 24.0,
        conclusion: Conclusion::Success,
        estimate: Some(20),
        steps: &[
            ("build", "Set up job"),
            ("build", "Checkout"),
            ("build", "Compile"),
            ("build", "Run tests"),
            ("build", "Upload artifact"),
        ],
    },
    Script {
        run_id: 1002,
        repo: "work-org/infra",
        workflow: "Deploy to staging",
        branch: "release/2026.8",
        sha: "9f8e7d6c5b4a3928",
        subject: "chore: bump the base image to 24.04",
        start: 5.0,
        finish: 30.0,
        conclusion: Conclusion::Failure,
        // No history: this card gets the pulsing indeterminate bar.
        estimate: None,
        steps: &[
            ("plan", "Terraform init"),
            ("plan", "Terraform plan"),
            ("deploy", "Apply"),
            ("deploy", "Smoke test"),
        ],
    },
    Script {
        run_id: 1003,
        repo: "me/dotfiles",
        workflow: "Lint",
        branch: "feature/very-long-branch-name-that-should-be-truncated",
        sha: "0011223344556677",
        subject: "wip",
        start: 9.0,
        finish: 21.0,
        conclusion: Conclusion::Cancelled,
        estimate: Some(30),
        steps: &[("shellcheck", "Scan scripts")],
    },
];

pub async fn run(snapshot_tx: watch::Sender<Arc<Snapshot>>, notify: Notify) {
    let linger = Duration::from_secs(8);
    let origin = Instant::now();
    tracing::info!("demo mode: replaying a scripted set of runs every {CYCLE}s");

    loop {
        let now = Instant::now();
        let t = now.duration_since(origin).as_secs_f64() % CYCLE;

        let runs: Vec<RunView> = SCRIPT
            .iter()
            .filter_map(|script| view_at(script, t, now))
            .collect();

        let watched = WATCHED
            .iter()
            .map(|(account, repo, age)| WatchedRepo {
                account: (*account).to_owned(),
                repo: (*repo).to_owned(),
                last_checked: now.checked_sub(Duration::from_secs(*age)),
                readable: true,
            })
            .collect();

        let snapshot = Snapshot {
            runs,
            issues: Vec::new(),
            watched,
            linger,
        };
        if snapshot_tx.send(Arc::new(snapshot)).is_err() {
            return; // the UI has gone away
        }
        notify();
        tokio::time::sleep(TICK).await;
    }
}

/// Build the card this script should produce at cycle time `t`, if any.
fn view_at(script: &Script, t: f64, now: Instant) -> Option<RunView> {
    // Keep finished runs around a little past the linger window, exactly as the
    // real poller does, so the UI is the thing that retires them.
    if t < script.start || t > script.finish + 12.0 {
        return None;
    }

    let running = t < script.finish;
    let elapsed = t - script.start;
    // The first two seconds are spent queued, waiting for a runner.
    let queued = elapsed < 2.0;

    let status = match (running, queued) {
        (false, _) => RunStatus::Completed,
        (true, true) => RunStatus::Queued,
        (true, false) => RunStatus::InProgress,
    };

    let current = (status == RunStatus::InProgress).then(|| {
        let span = (script.finish - script.start - 2.0).max(1.0);
        let fraction = ((t - script.start - 2.0) / span).clamp(0.0, 0.999);
        let index = (fraction * script.steps.len() as f64) as usize;
        let (job, step) = script.steps[index.min(script.steps.len() - 1)];
        JobProgress {
            job_name: job.to_owned(),
            step_name: Some(step.to_owned()),
            step_index: Some(index as u32 + 1),
            step_total: Some(script.steps.len() as u32),
        }
    });

    let finished_at = (!running).then(|| {
        let since_finish = Duration::from_secs_f64(t - script.finish);
        now.checked_sub(since_finish).unwrap_or(now)
    });

    Some(RunView {
        key: RunKey::new("demo", script.run_id),
        repo: script.repo.to_owned(),
        workflow: script.workflow.to_owned(),
        run_number: 100 + script.run_id % 100,
        branch: script.branch.to_owned(),
        head_sha: script.sha.to_owned(),
        commit_subject: script.subject.to_owned(),
        status,
        conclusion: (!running).then_some(script.conclusion),
        started_at: Utc::now() - chrono::Duration::milliseconds((elapsed * 1000.0) as i64),
        html_url: format!(
            "https://github.com/{}/actions/runs/{}",
            script.repo, script.run_id
        ),
        current,
        estimate: script.estimate.map(Duration::from_secs),
        finished_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_walks_a_run_through_every_state() {
        let now = Instant::now();
        let ci = &SCRIPT[0];

        assert!(view_at(ci, 0.0, now).is_none(), "not started yet");
        assert_eq!(
            view_at(ci, 3.0, now).expect("queued").status,
            RunStatus::Queued
        );
        assert_eq!(
            view_at(ci, 10.0, now).expect("running").status,
            RunStatus::InProgress
        );

        let done = view_at(ci, 25.0, now).expect("finished");
        assert_eq!(done.status, RunStatus::Completed);
        assert_eq!(done.conclusion, Some(Conclusion::Success));
        assert!(done.finished_at.is_some());

        assert!(view_at(ci, 45.0, now).is_none(), "retired");
    }

    #[test]
    fn step_progress_advances_without_running_off_the_end() {
        let now = Instant::now();
        let ci = &SCRIPT[0];
        let total = ci.steps.len() as u32;

        let early = view_at(ci, 5.0, now).expect("running").current.expect("step");
        let late = view_at(ci, 23.5, now).expect("running").current.expect("step");
        assert_eq!(early.step_index, Some(1));
        assert_eq!(late.step_index, Some(total));
        assert_eq!(late.step_total, Some(total));
    }

    #[test]
    fn one_scripted_run_has_no_history_so_the_bar_pulses() {
        assert!(
            SCRIPT.iter().any(|s| s.estimate.is_none()),
            "the demo must exercise the indeterminate bar"
        );
        assert!(SCRIPT.iter().any(|s| s.estimate.is_some()));
    }

    #[test]
    fn the_demo_watch_list_covers_more_than_one_account() {
        // The panel groups by account, so the demo must exercise that.
        let accounts: std::collections::HashSet<&str> =
            WATCHED.iter().map(|(account, _, _)| *account).collect();
        assert!(accounts.len() > 1);
    }

    #[test]
    fn the_cycle_ends_with_an_empty_snapshot_so_the_window_hides() {
        let now = Instant::now();
        let quiet = SCRIPT
            .iter()
            .filter_map(|script| view_at(script, CYCLE - 1.0, now))
            .count();
        assert_eq!(quiet, 0, "the window must be able to disappear each cycle");
    }
}
