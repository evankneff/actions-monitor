//! A very small slice of the GitHub REST API: listing workflow runs, their
//! jobs, and a user's recently-pushed repositories.
//!
//! Every GET goes through [`GithubClient::get`], which keeps the `ETag` of the
//! last response and replays it as `If-None-Match`. GitHub answers unchanged
//! resources with `304 Not Modified`, and 304s do not count against the hourly
//! rate limit, which is what makes minute-by-minute idle polling affordable.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;

const API_ROOT: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!("actions-monitor/", env!("CARGO_PKG_VERSION"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("authentication failed (401): GitHub rejected the token")]
    Unauthorized,
    #[error("access denied (403) for {resource}: {message}")]
    Forbidden { resource: String, message: String },
    #[error("{resource} was not found (404)")]
    NotFound { resource: String },
    #[error("rate limited; backing off for {}s", .retry_after.as_secs())]
    RateLimited { retry_after: Duration },
    #[error("GitHub returned {status} for {resource}: {message}")]
    Status {
        status: u16,
        resource: String,
        message: String,
    },
    #[error("network error talking to GitHub: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("could not parse the response for {resource}: {source}")]
    Decode {
        resource: String,
        #[source]
        source: serde_json::Error,
    },
}

impl ApiError {
    /// Whether retrying later, unchanged, could plausibly succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Transport(_) | Self::RateLimited { .. } => true,
            Self::Status { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Whether this permanently disqualifies the account until config changes.
    pub fn is_fatal_for_account(&self) -> bool {
        matches!(self, Self::Unauthorized)
    }
}

/// The rate-limit state reported alongside the most recent response.
#[derive(Debug, Clone, Copy, Default)]
pub struct RateInfo {
    pub remaining: Option<u32>,
    pub limit: Option<u32>,
    /// Unix timestamp at which the window resets.
    pub reset: Option<i64>,
}

impl RateInfo {
    fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let num = |name: &str| -> Option<u32> {
            headers.get(name)?.to_str().ok()?.trim().parse().ok()
        };
        Self {
            remaining: num("x-ratelimit-remaining"),
            limit: num("x-ratelimit-limit"),
            reset: headers
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok()),
        }
    }

    /// Time until the limit resets, clamped to something sane.
    pub fn time_until_reset(&self) -> Option<Duration> {
        let reset = self.reset?;
        let now = Utc::now().timestamp();
        let secs = (reset - now).clamp(0, 3600);
        Some(Duration::from_secs(secs as u64))
    }
}

/// A response body plus whether it came from the conditional-request cache.
#[derive(Debug)]
pub struct Fetched<T> {
    pub value: T,
    pub rate: RateInfo,
    /// `true` when GitHub answered `304 Not Modified` and we reused the body.
    pub not_modified: bool,
}

/// Per-account store of `ETag`s and the bodies they belong to.
///
/// Holding the raw body (rather than the parsed value) keeps this a single
/// non-generic type that can serve every endpoint.
#[derive(Debug, Default)]
pub struct CondCache {
    entries: HashMap<String, (String, String)>,
}

impl CondCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn etag(&self, url: &str) -> Option<&str> {
        self.entries.get(url).map(|(etag, _)| etag.as_str())
    }

    fn body(&self, url: &str) -> Option<&str> {
        self.entries.get(url).map(|(_, body)| body.as_str())
    }

    fn store(&mut self, url: &str, etag: String, body: String) {
        self.entries.insert(url.to_owned(), (etag, body));
    }

    fn forget(&mut self, url: &str) {
        self.entries.remove(url);
    }


    /// Drop per-run cache entries for runs we no longer follow, so a long
    /// session watching many short-lived runs does not grow without bound.
    /// Repository-level entries are always kept: those URLs are stable and
    /// their ETags are what make idle polling free.
    pub fn prune_finished_runs(&mut self, live: &std::collections::HashSet<u64>) {
        self.entries.retain(|url, _| {
            let Some(tail) = url.split("/actions/runs/").nth(1) else {
                return true;
            };
            match tail.split(['/', '?']).next().and_then(|id| id.parse::<u64>().ok()) {
                Some(run_id) => live.contains(&run_id),
                None => true,
            }
        });
    }
}

#[derive(Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    token: String,
}

impl GithubClient {
    pub fn new(token: String) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { http, token })
    }

    /// Conditional GET. On `304` the cached body is reparsed and returned.
    pub async fn get<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        resource: &str,
        cache: &mut CondCache,
    ) -> Result<Fetched<T>, ApiError> {
        let mut request = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION);
        if let Some(etag) = cache.etag(url) {
            request = request.header("If-None-Match", etag);
        }

        let response = request.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let rate = RateInfo::from_headers(&headers);

        if status == reqwest::StatusCode::NOT_MODIFIED {
            let Some(body) = cache.body(url) else {
                // We sent an If-None-Match we no longer have a body for. Drop
                // the ETag so the next poll fetches the resource in full.
                cache.forget(url);
                return Err(ApiError::Status {
                    status: 304,
                    resource: resource.to_owned(),
                    message: "cached body missing".into(),
                });
            };
            let value = serde_json::from_str(body).map_err(|source| ApiError::Decode {
                resource: resource.to_owned(),
                source,
            })?;
            return Ok(Fetched {
                value,
                rate,
                not_modified: true,
            });
        }

        if !status.is_success() {
            return Err(self.error_for(status, &headers, response, resource).await);
        }

        let etag = headers
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = response.text().await?;
        let value = serde_json::from_str(&body).map_err(|source| ApiError::Decode {
            resource: resource.to_owned(),
            source,
        })?;

        match etag {
            Some(etag) => cache.store(url, etag, body),
            // No ETag means no conditional request is possible next time.
            None => cache.forget(url),
        }

        Ok(Fetched {
            value,
            rate,
            not_modified: false,
        })
    }

    async fn error_for(
        &self,
        status: reqwest::StatusCode,
        headers: &reqwest::header::HeaderMap,
        response: reqwest::Response,
        resource: &str,
    ) -> ApiError {
        let rate = RateInfo::from_headers(headers);
        let retry_after = headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        // 403 and 429 both carry rate limiting; distinguish it from a genuine
        // permission problem by the remaining count and the Retry-After header.
        let exhausted = rate.remaining == Some(0);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || (exhausted && status.as_u16() == 403)
        {
            let wait = retry_after
                .or_else(|| rate.time_until_reset())
                .unwrap_or(Duration::from_secs(60));
            return ApiError::RateLimited {
                retry_after: wait.max(Duration::from_secs(30)),
            };
        }
        if let Some(wait) = retry_after {
            return ApiError::RateLimited { retry_after: wait };
        }

        let message = response
            .json::<ErrorBody>()
            .await
            .ok()
            .and_then(|b| b.message)
            .unwrap_or_else(|| status.to_string());

        match status.as_u16() {
            401 => ApiError::Unauthorized,
            403 => ApiError::Forbidden {
                resource: resource.to_owned(),
                message,
            },
            404 => ApiError::NotFound {
                resource: resource.to_owned(),
            },
            other => ApiError::Status {
                status: other,
                resource: resource.to_owned(),
                message,
            },
        }
    }

    /// Runs in a given state (`queued` / `in_progress`) for one repository.
    pub async fn list_runs(
        &self,
        repo: &str,
        status: &str,
        cache: &mut CondCache,
    ) -> Result<Fetched<RunsPage>, ApiError> {
        let url =
            format!("{API_ROOT}/repos/{repo}/actions/runs?status={status}&per_page=20&exclude_pull_requests=true");
        let resource = format!("{repo} {status} runs");
        self.get(&url, &resource, cache).await
    }

    pub async fn get_run(
        &self,
        repo: &str,
        run_id: u64,
        cache: &mut CondCache,
    ) -> Result<Fetched<WorkflowRun>, ApiError> {
        let url = format!("{API_ROOT}/repos/{repo}/actions/runs/{run_id}");
        let resource = format!("{repo} run {run_id}");
        self.get(&url, &resource, cache).await
    }

    pub async fn list_jobs(
        &self,
        repo: &str,
        run_id: u64,
        cache: &mut CondCache,
    ) -> Result<Fetched<JobsPage>, ApiError> {
        let url =
            format!("{API_ROOT}/repos/{repo}/actions/runs/{run_id}/jobs?filter=latest&per_page=50");
        let resource = format!("{repo} run {run_id} jobs");
        self.get(&url, &resource, cache).await
    }

    /// Repositories the token can see, most recently pushed first.
    pub async fn list_user_repos(
        &self,
        cache: &mut CondCache,
    ) -> Result<Fetched<Vec<Repository>>, ApiError> {
        let url = format!(
            "{API_ROOT}/user/repos?sort=pushed&direction=desc&per_page=100\
             &affiliation=owner,collaborator,organization_member"
        );
        self.get(&url, "your repositories", cache).await
    }
}

impl GithubClient {
    /// Who this token belongs to, and what it is allowed to do.
    pub async fn viewer(&self, cache: &mut CondCache) -> Result<Fetched<Viewer>, ApiError> {
        let url = format!("{API_ROOT}/user");
        let scopes = self.scopes_for(&url).await;
        let fetched: Fetched<UserBody> = self.get(&url, "your user account", cache).await?;
        Ok(Fetched {
            value: Viewer {
                login: fetched.value.login,
                scopes,
            },
            rate: fetched.rate,
            not_modified: fetched.not_modified,
        })
    }

    /// Read `x-oauth-scopes` with a cheap HEAD-style request.
    ///
    /// Kept separate from [`Self::get`] because that path deliberately discards
    /// headers, and because a missing scope header is not an error worth
    /// failing the whole check over.
    async fn scopes_for(&self, url: &str) -> Option<String> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .send()
            .await
            .ok()?;
        response
            .headers()
            .get("x-oauth-scopes")?
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

/// Pick out the repos pushed to within `days`, skipping ones that cannot run
/// Actions, and cap the list so the polling fan-out stays bounded.
pub fn recent_repo_slugs(repos: &[Repository], days: i64, max: usize, now: DateTime<Utc>) -> Vec<String> {
    let cutoff = now - chrono::Duration::days(days.max(1));
    let mut slugs: Vec<String> = repos
        .iter()
        .filter(|r| !r.archived && !r.disabled)
        .filter(|r| r.pushed_at.is_some_and(|pushed| pushed >= cutoff))
        .map(|r| r.full_name.clone())
        .collect();
    slugs.truncate(max);
    slugs
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunsPage {
    #[serde(default)]
    pub total_count: u64,
    #[serde(default)]
    pub workflow_runs: Vec<WorkflowRun>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub run_number: u64,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub run_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub head_commit: Option<HeadCommit>,
    #[serde(default)]
    pub repository: Option<RepositoryRef>,
}

impl WorkflowRun {
    pub fn started_at(&self) -> DateTime<Utc> {
        self.run_started_at.unwrap_or(self.created_at)
    }

    /// The first line of the head commit message, or an empty string.
    pub fn commit_subject(&self) -> String {
        self.head_commit
            .as_ref()
            .and_then(|c| c.message.lines().next())
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    pub fn workflow_name(&self) -> String {
        match self.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => "Workflow".to_owned(),
        }
    }

    pub fn branch(&self) -> String {
        self.head_branch.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HeadCommit {
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepositoryRef {
    #[serde(default)]
    pub full_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobsPage {
    #[serde(default)]
    pub jobs: Vec<Job>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub steps: Option<Vec<Step>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub number: Option<u32>,
}

/// The authenticated user, plus the scopes their token carries.
///
/// Classic tokens report scopes in the `x-oauth-scopes` response header, which
/// is the only way to see what one can actually do; fine-grained tokens do not
/// send the header at all.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub login: String,
    pub scopes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct UserBody {
    #[serde(default)]
    login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub pushed_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_runs_parse_with_missing_optional_fields() {
        let page: RunsPage = serde_json::from_str(
            r#"{"total_count":1,"workflow_runs":[{
                "id":42,"created_at":"2026-08-21T10:00:00Z","status":"in_progress"
            }]}"#,
        )
        .expect("parse");
        let run = &page.workflow_runs[0];
        assert_eq!(run.id, 42);
        assert_eq!(run.workflow_name(), "Workflow");
        assert_eq!(run.branch(), "");
        assert_eq!(run.commit_subject(), "");
        assert_eq!(run.started_at(), run.created_at);
    }

    #[test]
    fn run_started_at_wins_over_created_at() {
        let run: WorkflowRun = serde_json::from_str(
            r#"{"id":1,"created_at":"2026-08-21T10:00:00Z",
                "run_started_at":"2026-08-21T10:05:00Z",
                "head_commit":{"message":"fix: thing\n\nlong body here"}}"#,
        )
        .expect("parse");
        assert_eq!(run.started_at().to_rfc3339(), "2026-08-21T10:05:00+00:00");
        assert_eq!(run.commit_subject(), "fix: thing");
    }

    #[test]
    fn recent_repos_filter_by_push_date_and_cap() {
        let now = DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
            .expect("fixed timestamp")
            .with_timezone(&Utc);
        let repo = |name: &str, days_ago: i64, archived: bool| Repository {
            full_name: name.to_owned(),
            archived,
            disabled: false,
            pushed_at: Some(now - chrono::Duration::days(days_ago)),
        };
        let repos = vec![
            repo("me/fresh", 1, false),
            repo("me/archived", 1, true),
            repo("me/stale", 90, false),
            repo("me/also-fresh", 10, false),
            repo("me/third", 11, false),
        ];

        assert_eq!(
            recent_repo_slugs(&repos, 30, 10, now),
            vec!["me/fresh", "me/also-fresh", "me/third"]
        );
        assert_eq!(recent_repo_slugs(&repos, 30, 2, now).len(), 2, "capped");
        assert!(recent_repo_slugs(&repos, 30, 10, now).iter().all(|s| s != "me/stale"));
    }

    #[test]
    fn repos_without_a_push_date_are_skipped() {
        let now = Utc::now();
        let repos = vec![Repository {
            full_name: "me/never-pushed".into(),
            archived: false,
            disabled: false,
            pushed_at: None,
        }];
        assert!(recent_repo_slugs(&repos, 30, 10, now).is_empty());
    }

    #[test]
    fn rate_info_reads_the_standard_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "4321".parse().expect("header"));
        headers.insert("x-ratelimit-limit", "5000".parse().expect("header"));
        let rate = RateInfo::from_headers(&headers);
        assert_eq!(rate.remaining, Some(4321));
        assert_eq!(rate.limit, Some(5000));
        assert_eq!(rate.reset, None);
    }

    #[test]
    fn jobs_parse_and_expose_step_numbers() {
        let page: JobsPage = serde_json::from_str(
            r#"{"jobs":[{"name":"build","status":"in_progress","steps":[
                {"name":"Set up job","status":"completed","number":1},
                {"name":"Compile","status":"in_progress","number":3}]}]}"#,
        )
        .expect("parse");
        let steps = page.jobs[0].steps.as_ref().expect("steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].number, Some(3));
    }
}
