//! Exclusion patterns for repositories.
//!
//! Auto-discovery is deliberately blunt - everything you pushed to recently -
//! so there needs to be a way to say "except those". Patterns are written as
//! `owner/repo`, with `*` and `?` wildcards in either half:
//!
//! | Pattern | Excludes |
//! | --- | --- |
//! | `some-org/*` or `some-org` | every repo owned by `some-org` |
//! | `me/scratch` | exactly that repo |
//! | `me/experiment-*` | repos whose name starts with `experiment-` |
//! | `*/dotfiles` | a repo called `dotfiles` under any owner |
//!
//! Matching is case-insensitive, since GitHub treats owner and repo names that
//! way when resolving them.

/// One parsed `owner/repo` pattern, lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    owner: String,
    repo: String,
    /// The pattern as the user wrote it, for log and `--check` output.
    original: String,
}

/// The exclusion patterns for one account.
#[derive(Debug, Clone, Default)]
pub struct ExcludeList {
    patterns: Vec<Pattern>,
}

impl ExcludeList {
    /// Parse patterns, skipping any that are malformed.
    ///
    /// Validation rejects bad patterns at config-load time, so anything skipped
    /// here has already been reported to the user.
    pub fn parse(raw: &[String]) -> Self {
        Self {
            patterns: raw.iter().filter_map(|p| parse_pattern(p)).collect(),
        }
    }

    /// The first pattern excluding `slug`, if any. Returning the pattern rather
    /// than a bool lets callers say *why* a repo was dropped.
    pub fn matched_by(&self, slug: &str) -> Option<&str> {
        let (owner, repo) = split_slug(slug)?;
        let owner = owner.to_lowercase();
        let repo = repo.to_lowercase();
        self.patterns
            .iter()
            .find(|p| glob(&p.owner, &owner) && glob(&p.repo, &repo))
            .map(|p| p.original.as_str())
    }

    pub fn excludes(&self, slug: &str) -> bool {
        self.matched_by(slug).is_some()
    }

    /// Drop excluded entries, returning what survived and what was removed.
    pub fn partition(&self, slugs: Vec<String>) -> (Vec<String>, Vec<(String, String)>) {
        let mut kept = Vec::with_capacity(slugs.len());
        let mut dropped = Vec::new();
        for slug in slugs {
            match self.matched_by(&slug) {
                Some(pattern) => dropped.push((slug, pattern.to_owned())),
                None => kept.push(slug),
            }
        }
        (kept, dropped)
    }
}

/// Is this a usable pattern? Used by config validation for a clear error.
pub fn is_valid_pattern(raw: &str) -> bool {
    parse_pattern(raw).is_some()
}

/// `owner/repo`, or a bare `owner` as shorthand for `owner/*`.
fn parse_pattern(raw: &str) -> Option<Pattern> {
    // Only a trailing slash is forgiven: `some-org/` plainly means the whole
    // owner. A *leading* slash is left alone, because trimming it would silently
    // turn `/repo` - almost certainly a typo for `*/repo` - into `repo/*`, which
    // would exclude something entirely different without complaint.
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let (owner, repo) = match trimmed.split_once('/') {
        // "ignore this whole workspace" is the common case, so a bare name is
        // read as an owner rather than as a repo.
        None => (trimmed, "*"),
        Some((owner, repo)) => {
            if repo.contains('/') || owner.is_empty() || repo.is_empty() {
                return None;
            }
            (owner, repo)
        }
    };
    Some(Pattern {
        owner: owner.to_lowercase(),
        repo: repo.to_lowercase(),
        original: trimmed.to_owned(),
    })
}

fn split_slug(slug: &str) -> Option<(&str, &str)> {
    let (owner, repo) = slug.trim().split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then_some((owner, repo))
}

/// Wildcard match supporting `*` (any run of characters) and `?` (one
/// character), with backtracking so multiple stars behave.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    let (mut pi, mut ti) = (0usize, 0usize);
    // Where the most recent `*` was, and how much of the text it had consumed,
    // so a failed match can give the star one more character and retry.
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if let Some(star_at) = star {
            pi = star_at + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(patterns: &[&str]) -> ExcludeList {
        ExcludeList::parse(&patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn a_bare_owner_excludes_that_whole_workspace() {
        let ex = list(&["some-org"]);
        assert!(ex.excludes("some-org/project-one"));
        assert!(ex.excludes("some-org/project-two"));
        assert!(!ex.excludes("me/keeper"));
    }

    #[test]
    fn an_explicit_owner_wildcard_does_the_same() {
        let ex = list(&["some-org/*"]);
        assert!(ex.excludes("some-org/project-one"));
        assert!(!ex.excludes("other/project-one"));
    }

    #[test]
    fn an_exact_slug_excludes_only_that_repo() {
        let ex = list(&["me/scratch"]);
        assert!(ex.excludes("me/scratch"));
        assert!(!ex.excludes("me/scratchpad"));
        assert!(!ex.excludes("you/scratch"));
    }

    #[test]
    fn prefix_and_suffix_wildcards_work() {
        let ex = list(&["me/experiment-*", "*/dotfiles", "me/*-archive"]);
        assert!(ex.excludes("me/experiment-one"));
        assert!(ex.excludes("me/experiment-"));
        assert!(!ex.excludes("me/experiments"));
        assert!(ex.excludes("anyone/dotfiles"));
        assert!(ex.excludes("me/2024-archive"));
        assert!(!ex.excludes("me/archive-2024"));
    }

    #[test]
    fn matching_ignores_case_the_way_github_does() {
        let ex = list(&["Some-Org/PROJECT"]);
        assert!(ex.excludes("some-org/project"));
        assert!(ex.excludes("SOME-ORG/Project"));
    }

    #[test]
    fn the_matching_pattern_is_reported_so_the_reason_is_visible() {
        let ex = list(&["me/keep-me", "some-org"]);
        assert_eq!(ex.matched_by("some-org/project-one"), Some("some-org"));
        assert_eq!(ex.matched_by("me/keeper"), None);
    }

    #[test]
    fn partition_splits_and_explains() {
        let ex = list(&["some-org"]);
        let (kept, dropped) = ex.partition(vec![
            "me/keeper".into(),
            "some-org/project-one".into(),
            "some-org/project-two".into(),
        ]);
        assert_eq!(kept, vec!["me/keeper"]);
        assert_eq!(
            dropped,
            vec![
                ("some-org/project-one".to_owned(), "some-org".to_owned()),
                ("some-org/project-two".to_owned(), "some-org".to_owned()),
            ]
        );
    }

    #[test]
    fn an_empty_list_excludes_nothing() {
        let ex = ExcludeList::default();
        assert!(!ex.excludes("anyone/anything"));
        let (kept, dropped) = ex.partition(vec!["a/b".into()]);
        assert_eq!(kept, vec!["a/b"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn malformed_patterns_are_rejected() {
        assert!(!is_valid_pattern(""));
        assert!(!is_valid_pattern("   "));
        assert!(!is_valid_pattern("a/b/c"));
        // A leading slash is a typo, not a shorthand: rejected so the user is
        // told, rather than silently reinterpreted.
        assert!(!is_valid_pattern("/repo"));
        assert!(!is_valid_pattern("//"));

        // A trailing slash unambiguously means the whole owner.
        assert!(is_valid_pattern("owner/"));
        let ex = list(&["some-org/"]);
        assert!(ex.excludes("some-org/anything"));
        assert!(!ex.excludes("other/anything"));

        assert!(is_valid_pattern("owner"));
        assert!(is_valid_pattern("owner/repo"));
        assert!(is_valid_pattern("owner/*"));
        assert!(is_valid_pattern("*/repo"));
    }

    #[test]
    fn glob_handles_multiple_stars_and_needs_backtracking() {
        assert!(glob("*", "anything"));
        assert!(glob("*", ""));
        assert!(glob("a*b*c", "axxbyyc"));
        assert!(glob("*abc*", "xxabcyy"));
        assert!(!glob("a*b*c", "axxbyy"));
        assert!(glob("a?c", "abc"));
        assert!(!glob("a?c", "ac"));
        assert!(glob("**", "x"));
        assert!(!glob("abc", "abcd"));
        assert!(glob("abc", "abc"));
    }
}
