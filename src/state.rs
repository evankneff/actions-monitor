//! The UI-side model: a list of cards that is *diffed* against each incoming
//! snapshot rather than rebuilt.
//!
//! Rebuilding would reset per-card UI state (appear animation, ordering) every
//! five seconds and make the stack visibly flicker, so cards are matched by
//! [`RunKey`] and updated in place.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::model::{AccountIssue, RunKey, RunStatus, RunView, Snapshot, WatchedRepo};

/// How long an account issue (e.g. a rejected token) stays on screen.
pub const ISSUE_TTL: Duration = Duration::from_secs(30);
/// How long a notice raised by the UI itself (e.g. "config reloaded") lingers.
pub const NOTICE_TTL: Duration = Duration::from_secs(6);
/// How long the watched-repos panel stays up before closing itself.
pub const PANEL_TTL: Duration = Duration::from_secs(45);

/// A short message the UI raises for itself, in response to something the user
/// just did - as opposed to an [`AccountIssue`], which comes from the backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub is_error: bool,
    pub raised_at: Instant,
}

/// One rendered run card, plus the UI-only state that must survive updates.
#[derive(Clone, Debug)]
pub struct Card {
    pub view: RunView,
    /// When this card first appeared, used for the slide/fade-in animation.
    pub first_seen: Instant,
}

impl Card {
    /// Whether the card has outlived its post-completion linger.
    fn expired(&self, now: Instant, linger: Duration) -> bool {
        match self.view.finished_at {
            Some(finished) => now.saturating_duration_since(finished) >= linger,
            None => false,
        }
    }
}

#[derive(Debug, Default)]
pub struct AppState {
    cards: Vec<Card>,
    /// Runs the user closed with the card's X. They stay dismissed for as long
    /// as the backend keeps reporting them, even while still running.
    dismissed: HashSet<RunKey>,
    issues: Vec<AccountIssue>,
    dismissed_issues: HashSet<String>,
    /// Every issue the backend currently reports, ignoring dismissal and the
    /// on-screen timeout. The cards are deliberately transient, but the tray
    /// icon is a *status* indicator: an account that is still failing to poll
    /// must keep showing as broken long after its card has gone.
    live_issues: Vec<AccountIssue>,
    notices: Vec<Notice>,
    /// Everything the backend is polling, for the watched-repos panel.
    watched: Vec<WatchedRepo>,
    /// When the panel was opened from the tray, if it is open.
    panel_opened: Option<Instant>,
    linger: Duration,
}

impl AppState {
    pub fn new(linger: Duration) -> Self {
        Self {
            linger,
            ..Self::default()
        }
    }

    /// Merge a backend snapshot into the current card list.
    ///
    /// Existing cards are updated in place, runs that vanished are dropped, and
    /// genuinely new runs are appended in start order.
    pub fn apply(&mut self, snapshot: &Snapshot, now: Instant) {
        self.linger = snapshot.linger;

        let incoming: HashSet<&RunKey> = snapshot.runs.iter().map(|r| &r.key).collect();

        // A run we no longer hear about can never come back, so forget that it
        // was dismissed; otherwise the set grows for the life of the process.
        self.dismissed.retain(|key| incoming.contains(key));

        // Drop cards whose run left the snapshot.
        self.cards.retain(|card| incoming.contains(&card.view.key));

        // Update in place, collecting the ones we have not seen before.
        let mut fresh: Vec<&RunView> = Vec::new();
        for run in &snapshot.runs {
            if self.dismissed.contains(&run.key) {
                continue;
            }
            match self.cards.iter_mut().find(|c| c.view.key == run.key) {
                Some(card) => card.view = run.clone(),
                None => fresh.push(run),
            }
        }

        // Newly-seen runs join the stack oldest-first so the order is stable
        // regardless of the order the API happened to return them in.
        fresh.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.key.cmp(&b.key))
        });
        for run in fresh {
            self.cards.push(Card {
                view: run.clone(),
                first_seen: now,
            });
        }

        self.live_issues = snapshot.issues.clone();
        self.watched = snapshot.watched.clone();

        // Issues are keyed by account; a repeat report refreshes the existing one.
        self.dismissed_issues
            .retain(|account| snapshot.issues.iter().any(|i| &i.account == account));
        self.issues = snapshot
            .issues
            .iter()
            .filter(|issue| !self.dismissed_issues.contains(&issue.account))
            .cloned()
            .collect();
    }

    /// Forget expired cards. Called once per frame so the window can shrink and
    /// eventually hide itself.
    pub fn retire_expired(&mut self, now: Instant) {
        let linger = self.linger;
        self.cards.retain(|card| !card.expired(now, linger));
        self.issues
            .retain(|issue| now.saturating_duration_since(issue.raised_at) < ISSUE_TTL);
        self.notices
            .retain(|notice| now.saturating_duration_since(notice.raised_at) < NOTICE_TTL);
        if self
            .panel_opened
            .is_some_and(|at| now.saturating_duration_since(at) >= PANEL_TTL)
        {
            self.panel_opened = None;
        }
    }

    pub fn cards(&self) -> &[Card] {
        &self.cards
    }

    pub fn issues(&self) -> &[AccountIssue] {
        &self.issues
    }

    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    pub fn watched(&self) -> &[WatchedRepo] {
        &self.watched
    }

    /// Is the watched-repos panel currently showing?
    pub fn panel_open(&self) -> bool {
        self.panel_opened.is_some()
    }

    /// Open the panel, or close it if it is already open, so the tray item
    /// behaves as a toggle rather than piling panels up.
    pub fn toggle_panel(&mut self, now: Instant) {
        self.panel_opened = if self.panel_opened.is_some() {
            None
        } else {
            Some(now)
        };
    }

    pub fn close_panel(&mut self) {
        self.panel_opened = None;
    }

    /// Accounts the backend has given up on, whether or not their card is still
    /// showing. Drives the tray icon rather than the card stack.
    pub fn live_issues(&self) -> &[AccountIssue] {
        &self.live_issues
    }

    /// Raise a notice, replacing any previous one so repeated tray clicks do
    /// not stack up a column of near-identical cards.
    pub fn push_notice(&mut self, title: impl Into<String>, body: impl Into<String>, is_error: bool) {
        self.notices.clear();
        self.notices.push(Notice {
            title: title.into(),
            body: body.into(),
            is_error,
            raised_at: Instant::now(),
        });
    }

    pub fn dismiss_notice(&mut self, title: &str) {
        self.notices.retain(|notice| notice.title != title);
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
            && self.issues.is_empty()
            && self.notices.is_empty()
            && self.panel_opened.is_none()
    }

    /// Whether anything on screen is still moving, i.e. whether the UI needs to
    /// keep repainting rather than idling.
    pub fn has_live_run(&self) -> bool {
        self.cards.iter().any(|c| c.view.status.is_active())
    }

    pub fn dismiss(&mut self, key: &RunKey) {
        self.dismissed.insert(key.clone());
        self.cards.retain(|card| &card.view.key != key);
    }

    pub fn dismiss_issue(&mut self, account: &str) {
        self.dismissed_issues.insert(account.to_owned());
        self.issues.retain(|issue| issue.account != account);
    }

    /// The soonest moment at which [`Self::retire_expired`] would change
    /// anything, so a hidden window knows when to wake up.
    pub fn next_expiry(&self, now: Instant) -> Option<Duration> {
        let cards = self.cards.iter().filter_map(|card| {
            let finished = card.view.finished_at?;
            Some(
                self.linger
                    .saturating_sub(now.saturating_duration_since(finished)),
            )
        });
        let issues = self
            .issues
            .iter()
            .map(|issue| ISSUE_TTL.saturating_sub(now.saturating_duration_since(issue.raised_at)));
        let notices = self.notices.iter().map(|notice| {
            NOTICE_TTL.saturating_sub(now.saturating_duration_since(notice.raised_at))
        });
        let panel = self
            .panel_opened
            .map(|at| PANEL_TTL.saturating_sub(now.saturating_duration_since(at)));
        cards
            .chain(issues)
            .chain(notices)
            .chain(panel)
            .min()
    }
}

/// Short human summary of what a run is doing right now.
pub fn status_line(view: &RunView) -> String {
    match view.status {
        RunStatus::Queued => "Queued".to_owned(),
        RunStatus::InProgress => match &view.current {
            Some(progress) => {
                let mut line = progress.job_name.clone();
                if let Some(step) = &progress.step_name {
                    line.push_str(" \u{203a} ");
                    line.push_str(step);
                }
                if let (Some(index), Some(total)) = (progress.step_index, progress.step_total) {
                    line.push_str(&format!(" (step {index}/{total})"));
                }
                line
            }
            None => "Running".to_owned(),
        },
        RunStatus::Completed => view
            .conclusion
            .map_or_else(|| "Finished".to_owned(), |c| c.label().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Conclusion, IssueKind, JobProgress};
    use chrono::{TimeZone, Utc};

    fn snapshot(runs: Vec<RunView>) -> Snapshot {
        Snapshot {
            runs,
            issues: Vec::new(),
            watched: Vec::new(),
            linger: Duration::from_secs(8),
        }
    }

    fn run(id: u64, status: RunStatus) -> RunView {
        RunView {
            key: RunKey::new("personal", id),
            repo: "me/repo".into(),
            workflow: "CI".into(),
            run_number: id,
            branch: "main".into(),
            head_sha: "abcdef1234567890".into(),
            commit_subject: "fix things".into(),
            status,
            conclusion: None,
            started_at: Utc
                .timestamp_opt(1_700_000_000 + id as i64, 0)
                .single()
                .expect("valid timestamp"),
            html_url: format!("https://github.com/me/repo/actions/runs/{id}"),
            current: None,
            estimate: None,
            finished_at: None,
        }
    }

    fn ago(now: Instant, secs: u64) -> Instant {
        now.checked_sub(Duration::from_secs(secs)).unwrap_or(now)
    }

    #[test]
    fn new_runs_become_cards() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.apply(&snapshot(vec![run(1, RunStatus::Queued)]), now);

        assert_eq!(state.cards().len(), 1);
        assert_eq!(state.cards()[0].view.key, RunKey::new("personal", 1));
        assert!(!state.is_empty());
    }

    #[test]
    fn existing_cards_update_in_place_and_keep_their_identity() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.apply(&snapshot(vec![run(1, RunStatus::Queued)]), ago(now, 5));
        let first_seen = state.cards()[0].first_seen;

        let mut updated = run(1, RunStatus::InProgress);
        updated.current = Some(JobProgress {
            job_name: "build".into(),
            step_name: Some("Compile".into()),
            step_index: Some(3),
            step_total: Some(7),
        });
        state.apply(&snapshot(vec![updated]), now);

        assert_eq!(state.cards().len(), 1, "the card was updated, not replaced");
        assert_eq!(
            state.cards()[0].first_seen,
            first_seen,
            "first_seen survives the update so the appear animation does not restart"
        );
        assert_eq!(state.cards()[0].view.status, RunStatus::InProgress);
        assert_eq!(status_line(&state.cards()[0].view), "build \u{203a} Compile (step 3/7)");
    }

    #[test]
    fn runs_missing_from_a_snapshot_are_dropped() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.apply(
            &snapshot(vec![run(1, RunStatus::InProgress), run(2, RunStatus::Queued)]),
            now,
        );
        assert_eq!(state.cards().len(), 2);

        state.apply(&snapshot(vec![run(2, RunStatus::Queued)]), now);
        assert_eq!(state.cards().len(), 1);
        assert_eq!(state.cards()[0].view.key.run_id, 2);
    }

    #[test]
    fn cards_keep_a_stable_start_ordering() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        // Deliberately supply the newer run first; ordering must not follow the
        // order the API returned.
        state.apply(
            &snapshot(vec![run(9, RunStatus::InProgress), run(2, RunStatus::InProgress)]),
            now,
        );
        let ids: Vec<u64> = state.cards().iter().map(|c| c.view.key.run_id).collect();
        assert_eq!(ids, vec![2, 9]);

        // A later arrival appends rather than reshuffling the existing stack.
        state.apply(
            &snapshot(vec![
                run(9, RunStatus::InProgress),
                run(2, RunStatus::InProgress),
                run(5, RunStatus::Queued),
            ]),
            now,
        );
        let ids: Vec<u64> = state.cards().iter().map(|c| c.view.key.run_id).collect();
        assert_eq!(ids, vec![2, 9, 5]);
    }

    #[test]
    fn dismissed_runs_stay_dismissed_while_still_running() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.apply(&snapshot(vec![run(1, RunStatus::InProgress)]), now);
        state.dismiss(&RunKey::new("personal", 1));
        assert!(state.cards().is_empty());

        // The backend still reports the run; it must not come back.
        state.apply(&snapshot(vec![run(1, RunStatus::InProgress)]), now);
        assert!(state.cards().is_empty(), "a dismissed run must not reappear");

        // ...and neither should its completion.
        let mut done = run(1, RunStatus::Completed);
        done.conclusion = Some(Conclusion::Success);
        done.finished_at = Some(now);
        state.apply(&snapshot(vec![done]), now);
        assert!(state.cards().is_empty());
    }

    #[test]
    fn dismissal_is_forgotten_once_the_run_leaves_the_snapshot() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.apply(&snapshot(vec![run(1, RunStatus::InProgress)]), now);
        state.dismiss(&RunKey::new("personal", 1));

        state.apply(&snapshot(vec![]), now);
        // A brand new run that happens to reuse the id is a different run to us,
        // but more importantly the dismissal set must not leak.
        state.apply(&snapshot(vec![run(1, RunStatus::InProgress)]), now);
        assert_eq!(state.cards().len(), 1);
    }

    #[test]
    fn finished_cards_linger_then_retire() {
        let now = Instant::now();
        let linger = Duration::from_secs(8);
        let mut state = AppState::new(linger);

        let mut done = run(1, RunStatus::Completed);
        done.conclusion = Some(Conclusion::Success);
        done.finished_at = Some(ago(now, 3));
        state.apply(&snapshot(vec![done.clone()]), now);

        state.retire_expired(now);
        assert_eq!(state.cards().len(), 1, "still within the linger window");
        assert_eq!(state.next_expiry(now), Some(Duration::from_secs(5)));

        // Five seconds later the linger has elapsed.
        let later = now + Duration::from_secs(5);
        state.retire_expired(later);
        assert!(state.cards().is_empty());
        assert!(state.is_empty(), "the window can now hide");
    }

    #[test]
    fn a_running_card_never_expires() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.apply(&snapshot(vec![run(1, RunStatus::InProgress)]), now);

        state.retire_expired(now + Duration::from_secs(600));
        assert_eq!(state.cards().len(), 1);
        assert_eq!(state.next_expiry(now), None);
        assert!(state.has_live_run());
    }

    #[test]
    fn account_issues_show_once_and_can_be_dismissed() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        let mut snap = snapshot(vec![]);
        snap.issues.push(AccountIssue {
            account: "work".into(),
            kind: IssueKind::Auth,
            message: "token rejected".into(),
            raised_at: now,
        });

        state.apply(&snap, now);
        assert_eq!(state.issues().len(), 1);
        assert!(!state.is_empty(), "an issue alone is enough to show the window");

        state.dismiss_issue("work");
        assert!(state.issues().is_empty());

        // The backend keeps reporting it, but it stays dismissed.
        state.apply(&snap, now);
        assert!(state.issues().is_empty());
    }

    #[test]
    fn account_issues_time_out_on_their_own() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        let mut snap = snapshot(vec![]);
        snap.issues.push(AccountIssue {
            account: "work".into(),
            kind: IssueKind::Auth,
            message: "token rejected".into(),
            raised_at: ago(now, 5),
        });
        state.apply(&snap, now);

        state.retire_expired(now);
        assert_eq!(state.issues().len(), 1);

        state.retire_expired(now + ISSUE_TTL);
        assert!(state.issues().is_empty());
    }

    #[test]
    fn the_panel_toggles_and_keeps_the_window_up_on_its_own() {
        let mut state = AppState::new(Duration::from_secs(8));
        assert!(state.is_empty());

        let now = Instant::now();
        state.toggle_panel(now);
        assert!(state.panel_open());
        assert!(!state.is_empty(), "the panel alone should show the window");

        // Toggling again closes it, rather than opening a second one.
        state.toggle_panel(now);
        assert!(!state.panel_open());
        assert!(state.is_empty());
    }

    #[test]
    fn the_panel_closes_itself_eventually() {
        let now = Instant::now();
        let mut state = AppState::new(Duration::from_secs(8));
        state.toggle_panel(now);

        state.retire_expired(now);
        assert!(state.panel_open());
        assert_eq!(state.next_expiry(now), Some(PANEL_TTL));

        state.retire_expired(now + PANEL_TTL);
        assert!(!state.panel_open());
    }

    #[test]
    fn status_line_covers_every_run_state() {
        assert_eq!(status_line(&run(1, RunStatus::Queued)), "Queued");
        assert_eq!(status_line(&run(1, RunStatus::InProgress)), "Running");

        let mut partial = run(1, RunStatus::InProgress);
        partial.current = Some(JobProgress {
            job_name: "deploy".into(),
            step_name: None,
            step_index: None,
            step_total: None,
        });
        assert_eq!(status_line(&partial), "deploy");

        let mut failed = run(1, RunStatus::Completed);
        failed.conclusion = Some(Conclusion::Failure);
        assert_eq!(status_line(&failed), "Failed");
    }
}
