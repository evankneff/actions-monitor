//! The polling backend: one task per configured account, plus a supervisor that
//! merges their output into the snapshots the UI renders.
//!
//! # Cadence
//!
//! * **Idle** - every `poll_idle_seconds` each watched repo is asked for its
//!   `queued` and `in_progress` runs. These are conditional requests, so a quiet
//!   repo answers `304 Not Modified` and costs no rate limit.
//! * **Active** - the moment a run shows up, that *run* is refreshed every
//!   `poll_active_seconds` (the run itself plus its jobs, for the current
//!   job/step line). Other repos on the account stay on the idle cadence.
//! * **Backing off** - below 100 remaining requests the idle cadence drops to
//!   five minutes; `Retry-After` and exhausted-limit responses pause the account
//!   outright; network errors retry with exponential backoff capped at 2 minutes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::{mpsc, watch};

use crate::config::{AccountConfig, Config};
use crate::github::{self, ApiError, CondCache, GithubClient, Job};
use crate::history::History;
use crate::model::{
    AccountIssue, Conclusion, IssueKind, JobProgress, RunKey, RunStatus, RunView, Snapshot,
    WatchedRepo,
};

/// Rate-limit headroom below which we slow idle polling right down.
const LOW_RATE_THRESHOLD: u32 = 100;
/// Idle cadence used while the rate limit is nearly exhausted.
const LOW_RATE_IDLE: Duration = Duration::from_secs(300);
/// How often auto-discovery re-lists the account's repositories.
const DISCOVER_INTERVAL: Duration = Duration::from_secs(3600);
/// Upper bound on the retry backoff after transient failures.
const MAX_BACKOFF: Duration = Duration::from_secs(120);
/// Extra time a completed run is retained past the linger window, so the UI is
/// always the thing that decides when a card disappears.
const RETAIN_SLACK: Duration = Duration::from_secs(30);
/// Never sleep for less than this, so a misconfiguration cannot spin the loop.
const MIN_SLEEP: Duration = Duration::from_millis(250);
/// Cap on simultaneously tracked runs per account.
const MAX_TRACKED: usize = 32;

/// Called whenever a new snapshot is published, to wake the UI thread.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Timings {
    idle: Duration,
    active: Duration,
    linger: Duration,
}

impl Timings {
    fn from(config: &Config) -> Self {
        Self {
            idle: config.idle_interval(),
            active: config.active_interval(),
            linger: config.linger(),
        }
    }
}

/// One account's contribution to the global snapshot.
#[derive(Debug, Clone)]
struct AccountUpdate {
    account: String,
    runs: Vec<RunView>,
    issue: Option<AccountIssue>,
    watched: Vec<WatchedRepo>,
}

struct AccountTask {
    handle: tokio::task::JoinHandle<()>,
    signature: (AccountConfig, Timings),
}

/// Run the backend until the config channel closes.
pub async fn run(
    mut config_rx: watch::Receiver<Arc<Config>>,
    snapshot_tx: watch::Sender<Arc<Snapshot>>,
    history: Arc<Mutex<History>>,
    history_path: PathBuf,
    notify: Notify,
) {
    let (update_tx, mut update_rx) = mpsc::unbounded_channel::<AccountUpdate>();
    let mut tasks: HashMap<String, AccountTask> = HashMap::new();
    let mut outputs: HashMap<String, AccountUpdate> = HashMap::new();

    let mut config = config_rx.borrow_and_update().clone();
    reconcile(
        &config,
        &mut tasks,
        &mut outputs,
        &update_tx,
        &history,
        &history_path,
    );
    publish(&outputs, &config, &snapshot_tx, &notify);

    loop {
        tokio::select! {
            changed = config_rx.changed() => {
                if changed.is_err() {
                    break; // the UI is gone
                }
                config = config_rx.borrow_and_update().clone();
                reconcile(&config, &mut tasks, &mut outputs, &update_tx, &history, &history_path);
                publish(&outputs, &config, &snapshot_tx, &notify);
            }
            update = update_rx.recv() => {
                let Some(update) = update else { break };
                outputs.insert(update.account.clone(), update);
                publish(&outputs, &config, &snapshot_tx, &notify);
            }
        }
    }

    for task in tasks.values() {
        task.handle.abort();
    }
}

/// Start, stop and restart account tasks so they match the current config.
fn reconcile(
    config: &Config,
    tasks: &mut HashMap<String, AccountTask>,
    outputs: &mut HashMap<String, AccountUpdate>,
    update_tx: &mpsc::UnboundedSender<AccountUpdate>,
    history: &Arc<Mutex<History>>,
    history_path: &Path,
) {
    let timings = Timings::from(config);
    let wanted: HashSet<&str> = config.accounts.iter().map(|a| a.name.as_str()).collect();

    tasks.retain(|name, task| {
        if wanted.contains(name.as_str()) {
            true
        } else {
            tracing::info!(account = %name, "account removed from config; stopping poller");
            task.handle.abort();
            false
        }
    });
    outputs.retain(|name, _| wanted.contains(name.as_str()));

    for account in &config.accounts {
        let signature = (account.clone(), timings);
        if let Some(existing) = tasks.get(&account.name) {
            if existing.signature == signature && !existing.handle.is_finished() {
                continue; // unchanged and still running
            }
            existing.handle.abort();
        }

        let token = match account.resolve_token() {
            Ok(token) => token,
            Err(err) => {
                tracing::error!(account = %account.name, "{err}");
                outputs.insert(
                    account.name.clone(),
                    AccountUpdate {
                        account: account.name.clone(),
                        runs: Vec::new(),
                        watched: Vec::new(),
                        issue: Some(AccountIssue {
                            account: account.name.clone(),
                            kind: IssueKind::Auth,
                            message: err.to_string(),
                            raised_at: Instant::now(),
                        }),
                    },
                );
                tasks.remove(&account.name);
                continue;
            }
        };

        let client = match GithubClient::new(token) {
            Ok(client) => client,
            Err(err) => {
                tracing::error!(account = %account.name, "could not build an HTTP client: {err}");
                continue;
            }
        };

        tracing::info!(
            account = %account.name,
            auto_discover = account.auto_discover,
            repos = account.repos.len(),
            "starting poller"
        );

        let poller = AccountPoller::new(
            account.clone(),
            timings,
            client,
            update_tx.clone(),
            Arc::clone(history),
            history_path.to_path_buf(),
        );
        let handle = tokio::spawn(poller.run());
        tasks.insert(account.name.clone(), AccountTask { handle, signature });
    }
}

fn publish(
    outputs: &HashMap<String, AccountUpdate>,
    config: &Config,
    snapshot_tx: &watch::Sender<Arc<Snapshot>>,
    notify: &Notify,
) {
    let mut runs: Vec<RunView> = outputs
        .values()
        .flat_map(|update| update.runs.iter().cloned())
        .collect();
    runs.sort_by(|a, b| a.started_at.cmp(&b.started_at).then_with(|| a.key.cmp(&b.key)));
    let runs = dedupe_runs(runs);

    let mut issues: Vec<AccountIssue> = outputs.values().filter_map(|u| u.issue.clone()).collect();
    issues.sort_by(|a, b| a.account.cmp(&b.account));

    let mut watched: Vec<WatchedRepo> = outputs
        .values()
        .flat_map(|update| update.watched.iter().cloned())
        .collect();
    watched.sort_by(|a, b| a.account.cmp(&b.account).then_with(|| a.repo.cmp(&b.repo)));

    let snapshot = Snapshot {
        runs,
        issues,
        watched,
        linger: config.linger(),
    };
    if snapshot_tx.send(Arc::new(snapshot)).is_ok() {
        notify();
    }
}

/// Collapse runs that two accounts both reported.
///
/// The same repository can legitimately be watched twice - listed explicitly
/// under one account and picked up by auto-discovery under another, which is
/// easy to do by accident with several tokens. It is still one run, so it gets
/// one card. Whichever poller has the live job/step line wins, since that is
/// the more informative view; ties keep the earlier entry so the result is
/// stable frame to frame.
///
/// `input` must already be sorted, and identity is `(repo, run_id)` rather than
/// the account-scoped [`RunKey`]: GitHub run ids are unique across the service.
fn dedupe_runs(input: Vec<RunView>) -> Vec<RunView> {
    let mut index: HashMap<(String, u64), usize> = HashMap::new();
    let mut out: Vec<RunView> = Vec::with_capacity(input.len());

    for run in input {
        let identity = (run.repo.clone(), run.key.run_id);
        match index.get(&identity) {
            Some(&at) => {
                if out[at].current.is_none() && run.current.is_some() {
                    out[at] = run;
                }
            }
            None => {
                index.insert(identity, out.len());
                out.push(run);
            }
        }
    }
    out
}

/// A run we are actively following.
struct Tracked {
    repo: String,
    view: RunView,
    next_refresh: Instant,
    /// Guards against writing the same duration into the history twice.
    recorded: bool,
}

struct AccountPoller {
    config: AccountConfig,
    timings: Timings,
    client: GithubClient,
    cache: CondCache,
    tx: mpsc::UnboundedSender<AccountUpdate>,
    history: Arc<Mutex<History>>,
    history_path: PathBuf,

    repos: Vec<String>,
    /// Repos that answered 404/403; skipped until the next discovery pass.
    unusable_repos: HashSet<String>,
    /// When each repo was last polled, for the watched-repos panel.
    last_checked: HashMap<String, Instant>,
    tracked: HashMap<u64, Tracked>,

    next_sweep: Instant,
    next_discover: Instant,
    /// Set when GitHub explicitly told us to wait (429 / exhausted limit).
    paused_until: Option<Instant>,
    consecutive_failures: u32,
    rate_low: bool,
}

impl AccountPoller {
    fn new(
        config: AccountConfig,
        timings: Timings,
        client: GithubClient,
        tx: mpsc::UnboundedSender<AccountUpdate>,
        history: Arc<Mutex<History>>,
        history_path: PathBuf,
    ) -> Self {
        let now = Instant::now();
        let repos = config.watched_repos();
        Self {
            config,
            timings,
            client,
            cache: CondCache::new(),
            tx,
            history,
            history_path,
            repos,
            unusable_repos: HashSet::new(),
            last_checked: HashMap::new(),
            tracked: HashMap::new(),
            next_sweep: now,
            next_discover: now,
            paused_until: None,
            consecutive_failures: 0,
            rate_low: false,
        }
    }

    fn account(&self) -> &str {
        &self.config.name
    }

    async fn run(mut self) {
        loop {
            let now = Instant::now();

            if let Some(until) = self.paused_until {
                if until > now {
                    tokio::time::sleep_until(until.into()).await;
                    continue;
                }
                self.paused_until = None;
            }

            if self.config.auto_discover && self.next_discover <= now {
                match self.discover().await {
                    Ok(()) => self.next_discover = Instant::now() + DISCOVER_INTERVAL,
                    Err(err) => {
                        if self.handle_error(&err, "discovering repositories") {
                            return;
                        }
                        // Retry discovery on the normal backoff schedule.
                        self.next_discover = Instant::now() + self.backoff();
                    }
                }
            }

            if self.next_sweep <= Instant::now() {
                match self.sweep().await {
                    Ok(()) => {
                        self.consecutive_failures = 0;
                        self.next_sweep = Instant::now() + self.idle_interval();
                    }
                    Err(err) => {
                        if self.handle_error(&err, "listing workflow runs") {
                            return;
                        }
                        self.next_sweep = Instant::now() + self.backoff();
                    }
                }
            }

            if let Err(err) = self.refresh_active().await
                && self.handle_error(&err, "refreshing a run")
            {
                return;
            }

            self.prune(Instant::now());
            self.publish(self.coverage_issue());

            let deadline = self.next_deadline(Instant::now());
            tokio::time::sleep_until(deadline.into()).await;
        }
    }

    fn idle_interval(&self) -> Duration {
        if self.rate_low {
            LOW_RATE_IDLE.max(self.timings.idle)
        } else {
            self.timings.idle
        }
    }

    /// Exponential backoff: 5s, 10s, 20s, ... capped at [`MAX_BACKOFF`].
    fn backoff(&self) -> Duration {
        let exp = self.consecutive_failures.saturating_sub(1).min(6);
        Duration::from_secs(5u64 << exp).min(MAX_BACKOFF)
    }

    /// Returns `true` when the account should stop polling entirely.
    fn handle_error(&mut self, err: &ApiError, what: &str) -> bool {
        if err.is_fatal_for_account() {
            tracing::error!(account = %self.account(), "{what}: {err}; polling stopped until the config changes");
            self.publish(Some(AccountIssue {
                account: self.account().to_owned(),
                kind: IssueKind::Auth,
                // The card's heading already names the account.
                message: "GitHub rejected this token (401)".to_owned(),
                raised_at: Instant::now(),
            }));
            return true;
        }

        if let ApiError::RateLimited { retry_after } = err {
            tracing::warn!(account = %self.account(), "{what}: rate limited, pausing for {:?}", retry_after);
            self.paused_until = Some(Instant::now() + *retry_after);
            return false;
        }

        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if err.is_transient() {
            tracing::warn!(
                account = %self.account(),
                attempt = self.consecutive_failures,
                "{what}: {err}; retrying in {:?}", self.backoff()
            );
        } else {
            tracing::error!(account = %self.account(), "{what}: {err}");
        }
        false
    }

    fn note_rate(&mut self, rate: github::RateInfo) {
        let Some(remaining) = rate.remaining else {
            return;
        };
        let low = remaining < LOW_RATE_THRESHOLD;
        if low && !self.rate_low {
            tracing::warn!(
                account = %self.account(),
                remaining,
                limit = rate.limit.unwrap_or(0),
                resets_in_secs = rate.time_until_reset().map_or(0, |d| d.as_secs()),
                "GitHub rate limit is nearly exhausted; idle polling slowed to {}s",
                LOW_RATE_IDLE.as_secs()
            );
        } else if !low && self.rate_low {
            tracing::info!(account = %self.account(), remaining, "rate limit recovered");
        }
        self.rate_low = low;
    }

    async fn discover(&mut self) -> Result<(), ApiError> {
        let fetched = self.client.list_user_repos(&mut self.cache).await?;
        self.note_rate(fetched.rate);

        // Exclusions are applied before the cap, so ignoring a noisy org frees
        // up slots for repos you actually care about rather than wasting them.
        let excluded = self.config.exclusions();
        let (candidates, dropped) = excluded.partition(github::recent_repo_slugs(
            &fetched.value,
            self.config.discover_days,
            usize::MAX,
            Utc::now(),
        ));
        if !dropped.is_empty() && !fetched.not_modified {
            tracing::info!(
                account = %self.account(),
                count = dropped.len(),
                "excluded repos from auto-discovery"
            );
            for (slug, pattern) in &dropped {
                tracing::debug!(account = %self.account(), %slug, %pattern, "excluded");
            }
        }
        let mut slugs = candidates;
        slugs.truncate(self.config.discover_max_repos);
        if fetched.value.len() > slugs.len() && slugs.len() == self.config.discover_max_repos {
            tracing::info!(
                account = %self.account(),
                cap = self.config.discover_max_repos,
                "auto-discovery hit its repo cap; raise discover_max_repos to watch more"
            );
        }
        if !fetched.not_modified {
            tracing::info!(account = %self.account(), repos = slugs.len(), "auto-discovered repositories");
        }

        // A repo that reappears in discovery deserves another chance.
        self.unusable_repos.retain(|repo| !slugs.contains(repo));
        self.repos = slugs;
        Ok(())
    }

    /// Ask every watched repo for its queued and in-progress runs.
    ///
    /// Per-repo failures are absorbed here: a repo the token cannot see should
    /// not stop the other repos on the same account from being polled.
    async fn sweep(&mut self) -> Result<(), ApiError> {
        let repos: Vec<String> = self
            .repos
            .iter()
            .filter(|r| !self.unusable_repos.contains(*r))
            .cloned()
            .collect();

        if repos.is_empty() {
            return Ok(());
        }

        let mut fatal: Option<ApiError> = None;
        for repo in repos {
            for status in ["queued", "in_progress"] {
                match self.client.list_runs(&repo, status, &mut self.cache).await {
                    Ok(fetched) => {
                        self.note_rate(fetched.rate);
                        // A 304 counts as a successful check; that is the whole
                        // point of conditional requests.
                        self.last_checked.insert(repo.clone(), Instant::now());
                        if !fetched.not_modified {
                            for run in &fetched.value.workflow_runs {
                                self.track(&repo, run);
                            }
                        }
                    }
                    Err(err) if err.is_fatal_for_account() => {
                        fatal = Some(err);
                    }
                    Err(ApiError::NotFound { .. }) => {
                        tracing::warn!(
                            account = %self.account(), %repo,
                            "repository not found, or the token cannot see it; skipping"
                        );
                        self.unusable_repos.insert(repo.clone());
                    }
                    Err(ApiError::Forbidden { message, .. }) => {
                        tracing::warn!(
                            account = %self.account(), %repo,
                            "no Actions access ({message}); skipping"
                        );
                        self.unusable_repos.insert(repo.clone());
                    }
                    Err(err) => {
                        // Transient and rate-limit errors apply to the whole
                        // account, so stop the sweep and let the caller back off.
                        return Err(err);
                    }
                }
                if fatal.is_some() {
                    break;
                }
            }
            if fatal.is_some() {
                break;
            }
        }

        match fatal {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Start following a run we have not seen before, or refresh a known one's
    /// metadata from a list response.
    fn track(&mut self, repo: &str, run: &github::WorkflowRun) {
        let status = RunStatus::parse(run.status.as_deref().unwrap_or("queued"));
        if !status.is_active() {
            return;
        }
        if let Some(existing) = self.tracked.get(&run.id) {
            // Keep the live job/step line; only refresh the static metadata.
            let current = existing.view.current.clone();
            let finished_at = existing.view.finished_at;
            let view = self.build_view(repo, run, current, finished_at);
            if let Some(existing) = self.tracked.get_mut(&run.id) {
                existing.view = view;
            }
            return;
        }
        if self.tracked.len() >= MAX_TRACKED {
            tracing::warn!(
                account = %self.account(),
                "already tracking {MAX_TRACKED} runs; ignoring further runs this cycle"
            );
            return;
        }

        tracing::info!(
            account = %self.account(), %repo, run_id = run.id,
            workflow = %run.workflow_name(), "run started"
        );
        let view = self.build_view(repo, run, None, None);
        self.tracked.insert(
            run.id,
            Tracked {
                repo: repo.to_owned(),
                view,
                // Refresh immediately so the first card has a job/step line.
                next_refresh: Instant::now(),
                recorded: false,
            },
        );
    }

    fn build_view(
        &self,
        repo: &str,
        run: &github::WorkflowRun,
        current: Option<JobProgress>,
        finished_at: Option<Instant>,
    ) -> RunView {
        let repo = run
            .repository
            .as_ref()
            .map(|r| r.full_name.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(repo)
            .to_owned();
        let workflow = run.workflow_name();
        let estimate = self
            .history
            .lock()
            .ok()
            .and_then(|history| history.median(&History::key(&repo, &workflow)));

        RunView {
            key: RunKey::new(self.account(), run.id),
            repo,
            workflow,
            run_number: run.run_number,
            branch: run.branch(),
            head_sha: run.head_sha.clone(),
            commit_subject: run.commit_subject(),
            status: RunStatus::parse(run.status.as_deref().unwrap_or("queued")),
            conclusion: run.conclusion.as_deref().map(Conclusion::parse),
            started_at: run.started_at(),
            html_url: run.html_url.clone(),
            current,
            estimate,
            finished_at,
        }
    }

    /// Refresh each active run that is due, at the active cadence.
    async fn refresh_active(&mut self) -> Result<(), ApiError> {
        let now = Instant::now();
        let due: Vec<(u64, String)> = self
            .tracked
            .values()
            .filter(|t| t.view.status.is_active() && t.next_refresh <= now)
            .map(|t| (t.view.key.run_id, t.repo.clone()))
            .collect();

        for (run_id, repo) in due {
            match self.refresh_one(&repo, run_id).await {
                Ok(()) => {}
                Err(ApiError::NotFound { .. }) => {
                    tracing::info!(
                        account = %self.account(), %repo, run_id,
                        "run disappeared (deleted or expired); dropping it"
                    );
                    self.tracked.remove(&run_id);
                }
                Err(err) if err.is_fatal_for_account() => return Err(err),
                Err(err) => {
                    // Try again on the next tick rather than hammering.
                    if let Some(tracked) = self.tracked.get_mut(&run_id) {
                        tracked.next_refresh = Instant::now() + self.timings.active;
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    async fn refresh_one(&mut self, repo: &str, run_id: u64) -> Result<(), ApiError> {
        let fetched = self.client.get_run(repo, run_id, &mut self.cache).await?;
        self.note_rate(fetched.rate);
        let run = fetched.value;
        let status = RunStatus::parse(run.status.as_deref().unwrap_or("queued"));

        // The job/step line only matters while something is still executing.
        let current = if status.is_active() {
            match self.client.list_jobs(repo, run_id, &mut self.cache).await {
                Ok(jobs) => {
                    self.note_rate(jobs.rate);
                    derive_progress(&jobs.value.jobs)
                }
                Err(err) if err.is_fatal_for_account() => return Err(err),
                Err(err) => {
                    // The card is still useful without the step line.
                    tracing::debug!(%repo, run_id, "could not read jobs: {err}");
                    self.tracked.get(&run_id).and_then(|t| t.view.current.clone())
                }
            }
        } else {
            None
        };

        let previously_finished = self
            .tracked
            .get(&run_id)
            .and_then(|t| t.view.finished_at);
        let just_finished = previously_finished.is_none() && status == RunStatus::Completed;
        let finished_at = previously_finished.or_else(|| just_finished.then(Instant::now));

        let view = self.build_view(repo, &run, current, finished_at);

        if just_finished {
            tracing::info!(
                account = %self.account(), %repo, run_id,
                conclusion = run.conclusion.as_deref().unwrap_or("unknown"),
                "run finished"
            );
        }

        let record = just_finished && view.conclusion == Some(Conclusion::Success);
        let mut sample: Option<(String, String, Duration)> = None;

        if let Some(tracked) = self.tracked.get_mut(&run_id) {
            if record && !tracked.recorded {
                tracked.recorded = true;
                // Prefer GitHub's own timestamps over anything we measured, so
                // a restart mid-run still records the real duration.
                let duration = run
                    .updated_at
                    .map(|end| end - run.started_at())
                    .and_then(|d| d.to_std().ok());
                if let Some(duration) = duration {
                    sample = Some((view.repo.clone(), view.workflow.clone(), duration));
                }
            }
            tracked.view = view;
            tracked.next_refresh = Instant::now() + self.timings.active;
        }

        if let Some((repo, workflow, duration)) = sample {
            self.record_duration(&repo, &workflow, duration);
        }
        Ok(())
    }

    /// Append a successful run's duration to the on-disk history.
    fn record_duration(&self, repo: &str, workflow: &str, duration: Duration) {
        let key = History::key(repo, workflow);
        let Ok(mut history) = self.history.lock() else {
            tracing::warn!("history lock poisoned; skipping this sample");
            return;
        };
        history.record(&key, duration);
        tracing::debug!(%key, secs = duration.as_secs(), "recorded run duration");
        if let Err(err) = history.save(&self.history_path) {
            tracing::warn!("could not save run history: {err:#}");
        }
    }

    /// Forget completed runs once the UI has had its linger window, and keep
    /// the conditional-request cache from growing without bound.
    fn prune(&mut self, now: Instant) {
        let retain_for = self.timings.linger + RETAIN_SLACK;
        self.tracked.retain(|_, tracked| match tracked.view.finished_at {
            Some(finished) => now.saturating_duration_since(finished) < retain_for,
            None => true,
        });

        let live: HashSet<u64> = self.tracked.keys().copied().collect();
        self.cache.prune_finished_runs(&live);
    }

    /// An account whose every watched repo turned out to be unreadable is worth
    /// saying out loud once: it is almost always a token-scope mistake.
    fn coverage_issue(&self) -> Option<AccountIssue> {
        if self.repos.is_empty() || self.unusable_repos.len() < self.repos.len() {
            return None;
        }
        Some(AccountIssue {
            account: self.account().to_owned(),
            kind: IssueKind::Warning,
            message: format!(
                "none of the {} watched repos are readable \u{2014} check the token's Actions scope",
                self.repos.len()
            ),
            raised_at: Instant::now(),
        })
    }

    fn publish(&self, issue: Option<AccountIssue>) {
        let mut runs: Vec<RunView> = self.tracked.values().map(|t| t.view.clone()).collect();
        runs.sort_by_key(|run| run.started_at);

        let watched = self
            .repos
            .iter()
            .map(|repo| WatchedRepo {
                account: self.account().to_owned(),
                repo: repo.clone(),
                last_checked: self.last_checked.get(repo).copied(),
                readable: !self.unusable_repos.contains(repo),
            })
            .collect();

        // A closed channel just means the app is shutting down.
        let _ = self.tx.send(AccountUpdate {
            account: self.account().to_owned(),
            runs,
            issue,
            watched,
        });
    }

    /// The earliest moment at which this poller has something to do.
    fn next_deadline(&self, now: Instant) -> Instant {
        let mut deadline = self.next_sweep;
        if self.config.auto_discover {
            deadline = deadline.min(self.next_discover);
        }
        let retain_for = self.timings.linger + RETAIN_SLACK;
        for tracked in self.tracked.values() {
            match tracked.view.finished_at {
                None => deadline = deadline.min(tracked.next_refresh),
                Some(finished) => deadline = deadline.min(finished + retain_for),
            }
        }
        deadline.max(now + MIN_SLEEP)
    }
}

/// Work out which job and step a run is currently executing.
///
/// GitHub returns jobs in declaration order; the first one that is actually
/// running is the one worth showing. If nothing is running yet (everything is
/// still queued) we name the first queued job so the card is not blank.
pub fn derive_progress(jobs: &[Job]) -> Option<JobProgress> {
    let is = |job: &Job, want: &str| job.status.as_deref() == Some(want);

    let job = jobs
        .iter()
        .find(|j| is(j, "in_progress"))
        .or_else(|| jobs.iter().find(|j| is(j, "queued") || is(j, "waiting")))?;

    let steps = job.steps.as_deref().unwrap_or_default();
    let step_total = (!steps.is_empty()).then_some(steps.len() as u32);

    let running = steps
        .iter()
        .position(|s| s.status.as_deref() == Some("in_progress"));
    let (step_name, step_index) = match running {
        Some(index) => {
            let step = &steps[index];
            (
                Some(step.name.trim().to_owned()).filter(|n| !n.is_empty()),
                // `number` is GitHub's own 1-based ordinal; fall back to the
                // position in the list if it is ever missing.
                step.number.or(Some(index as u32 + 1)),
            )
        }
        None => (None, None),
    };

    Some(JobProgress {
        job_name: job.name.trim().to_owned(),
        step_name,
        step_index,
        step_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::JobsPage;

    fn jobs_from(json: &str) -> Vec<Job> {
        serde_json::from_str::<JobsPage>(json).expect("parse jobs").jobs
    }

    #[test]
    fn progress_points_at_the_running_job_and_step() {
        let jobs = jobs_from(
            r#"{"jobs":[
                {"name":"lint","status":"completed","conclusion":"success","steps":[
                    {"name":"Set up job","status":"completed","number":1}]},
                {"name":"build","status":"in_progress","steps":[
                    {"name":"Set up job","status":"completed","number":1},
                    {"name":"Checkout","status":"completed","number":2},
                    {"name":"Compile","status":"in_progress","number":3},
                    {"name":"Test","status":"queued","number":4},
                    {"name":"Package","status":"queued","number":5},
                    {"name":"Upload","status":"queued","number":6},
                    {"name":"Post job","status":"queued","number":7}]}]}"#,
        );
        let progress = derive_progress(&jobs).expect("a running job");
        assert_eq!(progress.job_name, "build");
        assert_eq!(progress.step_name.as_deref(), Some("Compile"));
        assert_eq!(progress.step_index, Some(3));
        assert_eq!(progress.step_total, Some(7));
    }

    #[test]
    fn progress_falls_back_to_a_queued_job_before_anything_runs() {
        let jobs = jobs_from(r#"{"jobs":[{"name":"deploy","status":"queued","steps":[]}]}"#);
        let progress = derive_progress(&jobs).expect("a queued job");
        assert_eq!(progress.job_name, "deploy");
        assert_eq!(progress.step_name, None);
        assert_eq!(progress.step_total, None, "an empty step list is not a total");
    }

    #[test]
    fn progress_is_none_when_every_job_has_finished() {
        let jobs = jobs_from(
            r#"{"jobs":[{"name":"build","status":"completed","conclusion":"success","steps":[]}]}"#,
        );
        assert!(derive_progress(&jobs).is_none());
        assert!(derive_progress(&[]).is_none());
    }

    #[test]
    fn progress_survives_a_job_whose_steps_are_absent() {
        let jobs = jobs_from(r#"{"jobs":[{"name":"build","status":"in_progress"}]}"#);
        let progress = derive_progress(&jobs).expect("a running job");
        assert_eq!(progress.job_name, "build");
        assert_eq!(progress.step_index, None);
        assert_eq!(progress.step_total, None);
    }

    fn view(account: &str, run_id: u64, repo: &str, step: Option<&str>) -> RunView {
        RunView {
            key: RunKey::new(account, run_id),
            repo: repo.to_owned(),
            workflow: "CI".into(),
            run_number: 1,
            branch: "main".into(),
            head_sha: "abcdef1234567".into(),
            commit_subject: "x".into(),
            status: RunStatus::InProgress,
            conclusion: None,
            started_at: Utc::now(),
            html_url: String::new(),
            current: step.map(|name| JobProgress {
                job_name: name.to_owned(),
                step_name: None,
                step_index: None,
                step_total: None,
            }),
            estimate: None,
            finished_at: None,
        }
    }

    #[test]
    fn a_repo_watched_by_two_accounts_yields_one_card() {
        // Exactly the two-token overlap: listed explicitly under `work`, and
        // auto-discovered under `personal`.
        let runs = vec![
            view("work", 500, "me/shared", None),
            view("personal", 500, "me/shared", None),
        ];
        let deduped = dedupe_runs(runs);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].key.account, "work", "the earlier entry is kept");
    }

    #[test]
    fn the_account_with_live_job_detail_wins() {
        let runs = vec![
            view("work", 500, "me/shared", None),
            view("personal", 500, "me/shared", Some("build")),
        ];
        let deduped = dedupe_runs(runs);
        assert_eq!(deduped.len(), 1);
        assert_eq!(
            deduped[0].current.as_ref().map(|c| c.job_name.as_str()),
            Some("build"),
            "the more informative view should survive"
        );

        // ...and the preference does not depend on ordering.
        let runs = vec![
            view("work", 500, "me/shared", Some("build")),
            view("personal", 500, "me/shared", None),
        ];
        let deduped = dedupe_runs(runs);
        assert_eq!(deduped.len(), 1);
        assert!(deduped[0].current.is_some());
    }

    #[test]
    fn genuinely_different_runs_are_all_kept() {
        let runs = vec![
            view("work", 500, "me/shared", None),
            view("work", 501, "me/shared", None),
            view("personal", 500, "me/other", None),
        ];
        assert_eq!(dedupe_runs(runs).len(), 3);
        assert!(dedupe_runs(Vec::new()).is_empty());
    }

    #[test]
    fn backoff_grows_exponentially_and_is_capped() {
        let mk = |failures: u32| {
            let exp = failures.saturating_sub(1).min(6);
            Duration::from_secs(5u64 << exp).min(MAX_BACKOFF)
        };
        assert_eq!(mk(1), Duration::from_secs(5));
        assert_eq!(mk(2), Duration::from_secs(10));
        assert_eq!(mk(3), Duration::from_secs(20));
        assert_eq!(mk(5), Duration::from_secs(80));
        assert_eq!(mk(6), MAX_BACKOFF, "capped at two minutes");
        assert_eq!(mk(50), MAX_BACKOFF);
    }
}
