//! Parsing, validation and hot-reloading of `config.toml`, found via `paths::data_dir`
//! (`%APPDATA%\actions-monitor` on Windows, `~/Library/Application Support/actions-monitor`
//! on macOS).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Written verbatim on first run, then opened in the default editor.
pub const TEMPLATE: &str = include_str!("config_template.toml");

fn default_idle_seconds() -> u64 {
    60
}
fn default_active_seconds() -> u64 {
    5
}
fn default_linger_seconds() -> u64 {
    8
}
fn default_show_tray_icon() -> bool {
    true
}
fn default_discover_days() -> i64 {
    30
}
fn default_discover_max_repos() -> usize {
    25
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_idle_seconds")]
    pub poll_idle_seconds: u64,
    #[serde(default = "default_active_seconds")]
    pub poll_active_seconds: u64,
    #[serde(default = "default_linger_seconds")]
    pub linger_seconds: u64,
    /// Show an icon in the notification area. Turn this off to go back to a
    /// completely invisible background process.
    #[serde(default = "default_show_tray_icon")]
    pub show_tray_icon: bool,
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_idle_seconds: default_idle_seconds(),
            poll_active_seconds: default_active_seconds(),
            linger_seconds: default_linger_seconds(),
            show_tray_icon: default_show_tray_icon(),
            accounts: Vec::new(),
        }
    }
}

impl Config {
    pub fn idle_interval(&self) -> Duration {
        Duration::from_secs(self.poll_idle_seconds.max(10))
    }

    pub fn active_interval(&self) -> Duration {
        Duration::from_secs(self.poll_active_seconds.max(2))
    }

    pub fn linger(&self) -> Duration {
        Duration::from_secs(self.linger_seconds)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub name: String,
    /// A literal token. Mutually exclusive with [`AccountConfig::token_env`].
    #[serde(default)]
    pub token: Option<String>,
    /// Name of an environment variable holding the token.
    #[serde(default)]
    pub token_env: Option<String>,
    /// Watch every repo pushed to recently instead of a fixed list.
    #[serde(default)]
    pub auto_discover: bool,
    #[serde(default)]
    pub repos: Vec<String>,
    /// Repositories never to watch, as `owner/repo` patterns with `*` and `?`
    /// wildcards. A bare `owner` means that whole owner. Applies to
    /// auto-discovered and explicitly listed repos alike.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// How far back "recently pushed" reaches, for `auto_discover`.
    #[serde(default = "default_discover_days")]
    pub discover_days: i64,
    /// Upper bound on auto-discovered repos, to keep the polling fan-out sane.
    #[serde(default = "default_discover_max_repos")]
    pub discover_max_repos: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("account `{0}` sets neither `token` nor `token_env`")]
    Missing(String),
    #[error("account `{0}` sets both `token` and `token_env`; pick one")]
    Ambiguous(String),
    #[error(
        "account `{account}` reads its token from `{var}`, but that environment variable is not set"
    )]
    EnvMissing { account: String, var: String },
    #[error("account `{account}` reads its token from `{var}`, but that variable is empty")]
    EnvEmpty { account: String, var: String },
    #[error(
        "account `{account}` has a GitHub token as the value of `token_env`. \
         `token_env` takes the NAME of an environment variable; to paste the \
         token itself, use `token = \"...\"` instead"
    )]
    TokenInEnvField { account: String },
}

/// Prefixes GitHub uses for its various credentials.
const TOKEN_PREFIXES: [&str; 6] = ["ghp_", "github_pat_", "gho_", "ghu_", "ghs_", "ghr_"];

/// Does this look like a secret rather than an environment variable name?
///
/// Worth checking explicitly: the two keys sit next to each other in the
/// template, and pasting a token into the wrong one otherwise fails with a
/// confusing "that environment variable is not set" - while writing the secret
/// into a field that gets logged and displayed as if it were a harmless name.
fn looks_like_a_token(value: &str) -> bool {
    let value = value.trim();
    TOKEN_PREFIXES
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

impl AccountConfig {
    /// Resolve the token, looking up environment variables through `lookup` so
    /// this is testable without mutating the process environment.
    pub fn resolve_token_with<F>(&self, lookup: F) -> Result<String, TokenError>
    where
        F: Fn(&str) -> Option<String>,
    {
        match (self.token.as_deref(), self.token_env.as_deref()) {
            (Some(_), Some(_)) => Err(TokenError::Ambiguous(self.name.clone())),
            (Some(token), None) => {
                let token = token.trim();
                if token.is_empty() {
                    Err(TokenError::Missing(self.name.clone()))
                } else {
                    Ok(token.to_owned())
                }
            }
            (None, Some(var)) if looks_like_a_token(var) => Err(TokenError::TokenInEnvField {
                account: self.name.clone(),
            }),
            (None, Some(var)) => match lookup(var) {
                None => Err(TokenError::EnvMissing {
                    account: self.name.clone(),
                    var: var.to_owned(),
                }),
                Some(value) if value.trim().is_empty() => Err(TokenError::EnvEmpty {
                    account: self.name.clone(),
                    var: var.to_owned(),
                }),
                Some(value) => Ok(value.trim().to_owned()),
            },
            (None, None) => Err(TokenError::Missing(self.name.clone())),
        }
    }

    pub fn resolve_token(&self) -> Result<String, TokenError> {
        self.resolve_token_with(|var| std::env::var(var).ok())
    }

    /// Explicitly listed repos, normalised, with obvious junk dropped and
    /// anything matching [`AccountConfig::exclude`] removed.
    pub fn watched_repos(&self) -> Vec<String> {
        let excluded = self.exclusions();
        self.repos
            .iter()
            .map(|r| r.trim().trim_matches('/').to_owned())
            .filter(|r| is_repo_slug(r))
            .filter(|r| !excluded.excludes(r))
            .collect()
    }

    /// The parsed exclusion patterns for this account.
    pub fn exclusions(&self) -> crate::filter::ExcludeList {
        crate::filter::ExcludeList::parse(&self.exclude)
    }
}

fn is_repo_slug(s: &str) -> bool {
    let mut parts = s.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) => !owner.is_empty() && !repo.is_empty(),
        _ => false,
    }
}

/// Parse and validate a config document.
pub fn parse(toml_str: &str) -> Result<Config> {
    let config: Config = toml::from_str(toml_str).context("config.toml is not valid TOML")?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<()> {
    let mut seen: Vec<&str> = Vec::new();
    for account in &config.accounts {
        let name = account.name.trim();
        if name.is_empty() {
            bail!("every [[accounts]] entry needs a non-empty `name`");
        }
        if seen.contains(&name) {
            bail!("two accounts are both named `{name}`; names must be unique");
        }
        seen.push(name);

        // Surface token misconfiguration at load time rather than at first poll.
        if account.token.is_some() && account.token_env.is_some() {
            bail!("{}", TokenError::Ambiguous(name.to_owned()));
        }
        if account.token.is_none() && account.token_env.is_none() {
            bail!("{}", TokenError::Missing(name.to_owned()));
        }
        if account
            .token_env
            .as_deref()
            .is_some_and(looks_like_a_token)
        {
            bail!("{}", TokenError::TokenInEnvField { account: name.to_owned() });
        }

        for repo in &account.repos {
            if !is_repo_slug(repo.trim()) {
                bail!("account `{name}` lists `{repo}`, which is not an `owner/repo` slug");
            }
        }
        for pattern in &account.exclude {
            if !crate::filter::is_valid_pattern(pattern) {
                bail!(
                    "account `{name}` excludes `{pattern}`, which is not a valid pattern;                      use `owner/repo`, `owner/*`, `*/repo` or a bare `owner`"
                );
            }
        }
        if !account.auto_discover && account.repos.is_empty() {
            bail!(
                "account `{name}` has `auto_discover = false` and an empty `repos` list, so it would watch nothing"
            );
        }
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<Config> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&raw)
}

/// Create the commented template if no config exists yet.
///
/// Returns `true` when a template was written, so the caller can tell the user
/// to fill it in and exit.
pub fn ensure_template(path: &Path) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

/// What a reload attempt did, so callers can report it to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reloaded {
    /// The file parsed and differed from what was loaded before.
    Changed { accounts: usize },
    /// The file parsed but is identical to the running config.
    Unchanged,
}

/// Re-reads `config.toml` on demand and publishes the result to the backend.
///
/// Shared between the filesystem watcher and the tray menu, so a manual
/// "Reload config" and a save-triggered reload go down exactly the same path.
pub struct Reloader {
    path: PathBuf,
    tx: tokio::sync::watch::Sender<Arc<Config>>,
}

impl Reloader {
    pub fn new(path: PathBuf, tx: tokio::sync::watch::Sender<Arc<Config>>) -> Self {
        Self { path, tx }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-read and republish the config.
    ///
    /// Errors are returned rather than logged-and-swallowed so the caller can
    /// decide whether to show them; the file watcher only logs, while the tray
    /// puts the parse error on screen.
    pub fn reload(&self) -> Result<Reloaded> {
        let config = Arc::new(load(&self.path)?);
        if self.tx.borrow().as_ref() == config.as_ref() {
            tracing::debug!("config re-read but unchanged");
            return Ok(Reloaded::Unchanged);
        }
        let accounts = config.accounts.len();
        tracing::info!(accounts, "config reloaded from disk");
        // A send error just means the backend has shut down.
        let _ = self.tx.send(config);
        Ok(Reloaded::Changed { accounts })
    }
}

/// Watch the config file for edits and reload when it changes.
///
/// Editors routinely save by writing a temp file and renaming over the target,
/// which fires as a delete + create rather than a modify, so we watch the
/// containing directory and filter by file name.
///
/// The returned watcher must be kept alive; dropping it stops the watch.
pub fn spawn_watcher(reloader: Arc<Reloader>) -> Result<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let dir = reloader
        .path
        .parent()
        .map(Path::to_path_buf)
        .context("config path has no parent directory")?;
    let file_name = reloader
        .path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .context("config path has no file name")?;

    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if event
            .paths
            .iter()
            .any(|p| p.file_name() == Some(&file_name))
        {
            // A send error only means the debounce thread has exited.
            let _ = raw_tx.send(());
        }
    })
    .context("creating the config file watcher")?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", dir.display()))?;

    std::thread::Builder::new()
        .name("config-watcher".into())
        .spawn(move || debounce_loop(&reloader, &raw_rx))
        .context("spawning the config debounce thread")?;

    Ok(watcher)
}

/// Coalesce bursts of filesystem events, then reload once things settle.
fn debounce_loop(reloader: &Reloader, raw_rx: &std::sync::mpsc::Receiver<()>) {
    const DEBOUNCE: Duration = Duration::from_millis(750);

    while raw_rx.recv().is_ok() {
        // Drain follow-up events belonging to the same save.
        while raw_rx.recv_timeout(DEBOUNCE).is_ok() {}

        if let Err(err) = reloader.reload() {
            // A half-written or briefly-invalid file is normal while editing.
            tracing::warn!("ignoring invalid config edit: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_ACCOUNTS: &str = r#"
poll_idle_seconds = 30
poll_active_seconds = 3
linger_seconds = 12

[[accounts]]
name = "personal"
token = "ghp_literal"
auto_discover = true
repos = []

[[accounts]]
name = "work"
token_env = "GH_TOKEN_WORK"
auto_discover = false
repos = ["work-org/app", "work-org/infra"]
"#;

    #[test]
    fn the_tray_icon_can_be_switched_off() {
        let config = parse(
            r#"
show_tray_icon = false

[[accounts]]
name = "solo"
token = "x"
auto_discover = true
"#,
        )
        .expect("parse");
        assert!(!config.show_tray_icon);
    }

    #[test]
    fn the_shipped_template_parses() {
        // The template ships fully commented out, so it must parse cleanly and
        // yield zero accounts (which is what triggers the first-run message).
        let config = parse(TEMPLATE).expect("template should parse");
        assert!(config.accounts.is_empty());
    }

    #[test]
    fn parses_two_accounts_with_overridden_timings() {
        let config = parse(TWO_ACCOUNTS).expect("parse");
        assert_eq!(config.poll_idle_seconds, 30);
        assert_eq!(config.active_interval(), Duration::from_secs(3));
        assert_eq!(config.linger(), Duration::from_secs(12));
        assert_eq!(config.accounts.len(), 2);

        let personal = &config.accounts[0];
        assert!(personal.auto_discover);
        assert_eq!(personal.discover_days, 30, "defaults apply per account");
        assert_eq!(personal.discover_max_repos, 25);

        let work = &config.accounts[1];
        assert!(!work.auto_discover);
        assert_eq!(work.watched_repos(), vec!["work-org/app", "work-org/infra"]);
    }

    #[test]
    fn omitted_timings_fall_back_to_documented_defaults() {
        let config = parse(
            r#"
[[accounts]]
name = "solo"
token = "x"
auto_discover = true
"#,
        )
        .expect("parse");
        assert_eq!(config.poll_idle_seconds, 60);
        assert_eq!(config.poll_active_seconds, 5);
        assert_eq!(config.linger_seconds, 8);
        assert!(config.show_tray_icon, "the tray icon is on unless asked otherwise");
    }

    #[test]
    fn literal_tokens_are_returned_verbatim() {
        let config = parse(TWO_ACCOUNTS).expect("parse");
        let token = config.accounts[0]
            .resolve_token_with(|_| panic!("must not consult the environment"))
            .expect("literal token");
        assert_eq!(token, "ghp_literal");
    }

    #[test]
    fn token_env_reads_the_named_variable() {
        let config = parse(TWO_ACCOUNTS).expect("parse");
        let work = &config.accounts[1];

        let token = work
            .resolve_token_with(|var| {
                assert_eq!(var, "GH_TOKEN_WORK");
                Some("  ghp_from_env  ".to_owned())
            })
            .expect("env token");
        assert_eq!(token, "ghp_from_env", "surrounding whitespace is trimmed");
    }

    #[test]
    fn token_env_reports_which_variable_is_missing() {
        let config = parse(TWO_ACCOUNTS).expect("parse");
        let err = config.accounts[1]
            .resolve_token_with(|_| None)
            .expect_err("should fail");
        assert_eq!(
            err,
            TokenError::EnvMissing {
                account: "work".into(),
                var: "GH_TOKEN_WORK".into()
            }
        );

        let err = config.accounts[1]
            .resolve_token_with(|_| Some("   ".to_owned()))
            .expect_err("should fail");
        assert!(matches!(err, TokenError::EnvEmpty { .. }));
    }

    #[test]
    fn a_token_pasted_into_token_env_is_caught_by_name() {
        // The easy mistake: `token_env` takes a variable NAME, but the token
        // itself is what is on your clipboard. Left unchecked this fails later
        // with a baffling "environment variable is not set".
        for value in [
            "ghp_EXAMPLEONLY000000000000000000000000",
            "github_pat_11ABCDEFG0abcdefghij",
            "  ghs_abcdefghijklmnop  ",
        ] {
            let toml = format!(
                r#"
[[accounts]]
name = "work"
token_env = "{value}"
repos = ["o/r"]
"#
            );
            let err = parse(&toml).expect_err("a token in token_env must be rejected");
            let message = err.to_string();
            assert!(message.contains("token_env"), "{message}");
            assert!(
                !message.contains(value.trim()),
                "the error must not echo the secret back: {message}"
            );
        }
    }

    #[test]
    fn ordinary_variable_names_are_still_accepted() {
        for name in ["GH_TOKEN_WORK", "MY_TOKEN", "gh_token_personal"] {
            let account = AccountConfig {
                name: "a".into(),
                token: None,
                token_env: Some(name.to_owned()),
                auto_discover: true,
                repos: Vec::new(),
                exclude: Vec::new(),
                discover_days: 30,
                discover_max_repos: 25,
            };
            assert_eq!(
                account.resolve_token_with(|_| Some("secret".into())),
                Ok("secret".to_owned()),
                "{name} should be treated as a variable name"
            );
        }
    }

    #[test]
    fn rejects_accounts_with_both_or_neither_token_source() {
        let both = parse(
            r#"
[[accounts]]
name = "a"
token = "x"
token_env = "Y"
repos = ["o/r"]
"#,
        );
        assert!(both.is_err(), "both token and token_env must be rejected");

        let neither = parse(
            r#"
[[accounts]]
name = "a"
repos = ["o/r"]
"#,
        );
        assert!(neither.is_err(), "a token source is required");
    }

    #[test]
    fn rejects_duplicate_names_and_malformed_repo_slugs() {
        assert!(
            parse(
                r#"
[[accounts]]
name = "dup"
token = "x"
repos = ["o/r"]

[[accounts]]
name = "dup"
token = "y"
repos = ["o/r2"]
"#
            )
            .is_err()
        );

        assert!(
            parse(
                r#"
[[accounts]]
name = "a"
token = "x"
repos = ["not-a-slug"]
"#
            )
            .is_err()
        );
    }

    #[test]
    fn exclusions_apply_to_explicitly_listed_repos_too() {
        let config = parse(
            r#"
[[accounts]]
name = "work"
token = "x"
auto_discover = false
repos = ["org/keep", "org/drop", "other/keep"]
exclude = ["org/drop"]
"#,
        )
        .expect("parse");
        assert_eq!(
            config.accounts[0].watched_repos(),
            vec!["org/keep", "other/keep"]
        );
    }

    #[test]
    fn a_bare_owner_exclusion_parses_and_applies() {
        let config = parse(
            r#"
[[accounts]]
name = "personal"
token = "x"
auto_discover = true
exclude = ["some-org", "me/scratch-*"]
"#,
        )
        .expect("parse");
        let excluded = config.accounts[0].exclusions();
        assert!(excluded.excludes("some-org/anything"));
        assert!(excluded.excludes("me/scratch-1"));
        assert!(!excluded.excludes("me/keeper"));
    }

    #[test]
    fn rejects_malformed_exclusion_patterns() {
        let err = parse(
            r#"
[[accounts]]
name = "a"
token = "x"
auto_discover = true
exclude = ["a/b/c"]
"#,
        )
        .expect_err("should fail");
        assert!(err.to_string().contains("not a valid pattern"), "{err}");
    }

    #[test]
    fn rejects_an_account_that_would_watch_nothing() {
        let err = parse(
            r#"
[[accounts]]
name = "idle"
token = "x"
auto_discover = false
repos = []
"#,
        )
        .expect_err("should fail");
        assert!(err.to_string().contains("watch nothing"));
    }

    #[test]
    fn rejects_unknown_keys_so_typos_are_not_silently_ignored() {
        let err = parse("poll_idle_secconds = 60\n").expect_err("typo should fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown") || msg.contains("valid TOML"), "{msg}");
    }
}
