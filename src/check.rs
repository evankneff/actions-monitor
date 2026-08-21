//! The `--check` command: a dry run of everything the poller would do, printed
//! to the console instead of acted on.
//!
//! It answers the three questions that come up whenever an account is added -
//! is the token valid, which repositories will auto-discovery actually watch,
//! and can Actions be read on each of them - without waiting for a workflow to
//! run or reading through the log.

use std::fmt::Write as _;

use chrono::{DateTime, Utc};

use crate::config::{AccountConfig, Config};
use crate::github::{self, ApiError, CondCache, GithubClient, Repository};

/// Width the repo names are padded to, for a readable column of results.
const NAME_WIDTH: usize = 44;

/// Run the check. Returns `true` if every account looked healthy.
pub async fn run(config: &Config) -> bool {
    let mut all_ok = true;

    for (index, account) in config.accounts.iter().enumerate() {
        if index > 0 {
            println!();
        }
        if !check_account(account).await {
            all_ok = false;
        }
    }

    println!();
    if all_ok {
        println!("All accounts look healthy.");
    } else {
        println!("Some accounts have problems; see above.");
    }
    all_ok
}

async fn check_account(account: &AccountConfig) -> bool {
    println!("account \"{}\"", account.name);

    let token = match account.resolve_token() {
        Ok(token) => token,
        Err(err) => {
            println!("  token           : UNUSABLE - {err}");
            return false;
        }
    };
    let source = match (&account.token, &account.token_env) {
        (Some(_), _) => "literal token in config.toml".to_owned(),
        (_, Some(var)) => format!("environment variable {var}"),
        _ => "unknown".to_owned(),
    };
    println!("  token           : {} ({source})", describe(&token));

    let client = match GithubClient::new(token) {
        Ok(client) => client,
        Err(err) => {
            println!("  token           : could not build an HTTP client - {err}");
            return false;
        }
    };
    let mut cache = CondCache::new();

    // Identity and scopes: the fastest way to see what a token really is.
    match client.viewer(&mut cache).await {
        Ok(fetched) => {
            println!("  identity        : {}", fetched.value.login);
            match fetched.value.scopes.as_deref() {
                Some(scopes) if !scopes.is_empty() => println!("  scopes          : {scopes}"),
                Some(_) => println!("  scopes          : none"),
                None => println!("  scopes          : n/a (fine-grained tokens do not report them)"),
            }
            if let Some(remaining) = fetched.rate.remaining {
                let limit = fetched.rate.limit.unwrap_or(0);
                let resets = fetched
                    .rate
                    .time_until_reset()
                    .map_or_else(String::new, |d| format!(", resets in {}m", d.as_secs() / 60));
                println!("  rate limit      : {remaining}/{limit} remaining{resets}");
            }
        }
        Err(err) => {
            println!("  identity        : FAILED - {}", explain(&err));
            return false;
        }
    }

    // Which repositories will actually be watched?
    let repos = if account.auto_discover {
        match discover(&client, account, &mut cache).await {
            Some(repos) => repos,
            None => return false,
        }
    } else {
        let repos = account.watched_repos();
        println!("  auto-discover   : off");
        println!("  watching        : {} repo(s) from the config", repos.len());
        repos
    };

    if repos.is_empty() {
        println!("  RESULT          : nothing to watch");
        return false;
    }

    // And can Actions be read on each of them?
    println!("  Actions access:");
    let mut ok = true;
    for repo in &repos {
        let verdict = match client.list_runs(repo, "in_progress", &mut cache).await {
            Ok(fetched) => match fetched.value.total_count {
                0 => "OK".to_owned(),
                n => format!("OK ({n} run(s) in progress now)"),
            },
            Err(err) => {
                ok = false;
                explain(&err)
            }
        };
        println!("    {:<NAME_WIDTH$} {verdict}", format!("{repo} "));
    }
    ok
}

/// List what auto-discovery would pick, and say what it left behind.
async fn discover(
    client: &GithubClient,
    account: &AccountConfig,
    cache: &mut CondCache,
) -> Option<Vec<String>> {
    println!(
        "  auto-discover   : on (pushed within {} days, at most {})",
        account.discover_days, account.discover_max_repos
    );

    let all: Vec<Repository> = match client.list_user_repos(cache).await {
        Ok(fetched) => fetched.value,
        Err(err) => {
            println!("  discovery       : FAILED - {}", explain(&err));
            return None;
        }
    };

    let now = Utc::now();
    // Mirror the poller exactly: exclude first, then apply the cap.
    let recent = github::recent_repo_slugs(&all, account.discover_days, usize::MAX, now);
    let (mut chosen, excluded) = account.exclusions().partition(recent);
    let over_cap = chosen.len().saturating_sub(account.discover_max_repos);
    chosen.truncate(account.discover_max_repos);

    println!("  visible to token: {} repo(s)", all.len());
    if !excluded.is_empty() {
        println!("  excluded        : {} repo(s):", excluded.len());
        for (slug, pattern) in &excluded {
            println!(
                "    {:<NAME_WIDTH$} matched exclude \"{pattern}\"",
                format!("{slug} ")
            );
        }
    }
    println!("  watching        : {} repo(s):", chosen.len());
    for slug in &chosen {
        let pushed = all
            .iter()
            .find(|r| &r.full_name == slug)
            .and_then(|r| r.pushed_at)
            .map_or_else(|| "unknown".to_owned(), |at| ago(at, now));
        println!("    {:<NAME_WIDTH$} last push {pushed}", format!("{slug} "));
    }

    // Being explicit about what was dropped is the point of the command: a repo
    // silently missing from the watch list is the confusing case.
    let archived = all.iter().filter(|r| r.archived || r.disabled).count();
    let stale = all
        .iter()
        .filter(|r| !r.archived && !r.disabled)
        .filter(|r| {
            r.pushed_at
                .is_none_or(|at| at < now - chrono::Duration::days(account.discover_days.max(1)))
        })
        .count();

    let mut reasons = String::new();
    let _ = write!(reasons, "{stale} not pushed recently, {archived} archived or disabled");
    if !excluded.is_empty() {
        let _ = write!(reasons, ", {} excluded", excluded.len());
    }
    if over_cap > 0 {
        let _ = write!(reasons, ", {over_cap} over the discover_max_repos cap");
    }
    let skipped = all.len() - chosen.len();
    if skipped > 0 {
        println!("  skipped         : {skipped} ({reasons})");
    }
    if over_cap > 0 {
        println!(
            "  NOTE            : {over_cap} repo(s) dropped by discover_max_repos = {}; \
             raise it or exclude what you do not need",
            account.discover_max_repos
        );
    }

    Some(chosen)
}

/// Classify a token by prefix, without ever showing it.
fn describe(token: &str) -> &'static str {
    if token.starts_with("github_pat_") {
        "fine-grained"
    } else if token.starts_with("ghp_") {
        "classic"
    } else if token.starts_with("gho_") || token.starts_with("ghu_") {
        "OAuth"
    } else {
        "unrecognised type"
    }
}

/// Turn an API error into something a person can act on.
fn explain(err: &ApiError) -> String {
    match err {
        ApiError::Unauthorized => {
            "401 - the token was rejected (revoked, expired or mistyped)".to_owned()
        }
        ApiError::NotFound { .. } => {
            "404 - not visible to this token (wrong scope, not a member, or the name is wrong)"
                .to_owned()
        }
        ApiError::Forbidden { message, .. } => {
            let hint = if message.to_lowercase().contains("saml") {
                " [authorise this token for the org: Configure SSO on the token page]"
            } else {
                ""
            };
            format!("403 - {message}{hint}")
        }
        other => other.to_string(),
    }
}

/// Rough, readable age of a timestamp.
fn ago(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = (now - at).num_minutes().max(0);
    match minutes {
        0..=59 => format!("{minutes}m ago"),
        60..=1439 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

/// Print a heading for the whole run.
pub fn header(path: &std::path::Path, config: &Config) {
    println!("actions-monitor --check");
    println!("config: {}", path.display());
    println!(
        "{} account(s); polling every {}s idle, {}s while a run is active",
        config.accounts.len(),
        config.poll_idle_seconds,
        config.poll_active_seconds
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_classified_by_prefix_and_never_echoed() {
        assert_eq!(describe("ghp_abc123"), "classic");
        assert_eq!(describe("github_pat_abc123"), "fine-grained");
        assert_eq!(describe("gho_abc"), "OAuth");
        assert_eq!(describe("something-else"), "unrecognised type");
        // Every branch returns a fixed label, so a secret can never leak here.
        for token in ["ghp_secret", "github_pat_secret", "secret"] {
            assert!(!describe(token).contains("secret"));
        }
    }

    #[test]
    fn ages_read_naturally() {
        let now = Utc::now();
        assert_eq!(ago(now - chrono::Duration::minutes(5), now), "5m ago");
        assert_eq!(ago(now - chrono::Duration::hours(3), now), "3h ago");
        assert_eq!(ago(now - chrono::Duration::days(9), now), "9d ago");
        // A clock skew into the future must not underflow.
        assert_eq!(ago(now + chrono::Duration::hours(1), now), "0m ago");
    }

    #[test]
    fn saml_failures_get_an_actionable_hint() {
        let err = ApiError::Forbidden {
            resource: "org/repo".into(),
            message: "Resource protected by organization SAML enforcement".into(),
        };
        let text = explain(&err);
        assert!(text.contains("403"));
        assert!(text.contains("Configure SSO"), "{text}");

        let plain = ApiError::Forbidden {
            resource: "org/repo".into(),
            message: "Forbidden".into(),
        };
        assert!(!explain(&plain).contains("Configure SSO"));
    }

    #[test]
    fn unauthorised_and_not_found_are_distinguished() {
        assert!(explain(&ApiError::Unauthorized).contains("401"));
        assert!(
            explain(&ApiError::NotFound {
                resource: "o/r".into()
            })
            .contains("404")
        );
    }
}
