//! Access policy.
//!
//! Defines the GitHub policy data model and the repository pattern matching
//! used to narrow a GitHub token's scope. A pattern is either an exact
//! `owner/repo`, an owner wildcard `org/*`, or the catch-all `*`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::secrets::Permission;

/// The curated set of GitHub fine-grained permissions the gateway enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    Contents,
    PullRequests,
    Issues,
    Actions,
    Metadata,
}

impl Scope {
    /// Map a canonical lowercase permission name to its [`Scope`].
    pub fn from_name(name: &str) -> Option<Scope> {
        match name {
            "contents" => Some(Scope::Contents),
            "pull_requests" => Some(Scope::PullRequests),
            "issues" => Some(Scope::Issues),
            "actions" => Some(Scope::Actions),
            "metadata" => Some(Scope::Metadata),
            _ => None,
        }
    }
}

/// A compiled repository rule: a pattern plus the permissions it grants.
///
/// A scope missing from the permissions map is treated as `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRepoRule {
    pub pattern: String,
    pub permissions: BTreeMap<Scope, Permission>,
}

/// A per-repository GitHub policy, an ordered list of compiled rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubPolicy {
    rules: Vec<CompiledRepoRule>,
}

impl GithubPolicy {
    /// Build a policy from already compiled rules.
    pub fn new(rules: Vec<CompiledRepoRule>) -> GithubPolicy {
        GithubPolicy { rules }
    }
}

/// Return whether a repository pattern matches an owner and repository name.
///
/// `owner/repo` matches exactly, `org/*` matches every repository owned by
/// `org`, and `*` matches every repository.
pub fn repo_matches(pattern: &str, owner: &str, repo: &str) -> bool {
    match pattern {
        "*" => true,
        _ => match pattern.split_once('/') {
            Some((pattern_owner, "*")) => pattern_owner == owner && !repo.is_empty(),
            Some((pattern_owner, pattern_repo)) => pattern_owner == owner && pattern_repo == repo,
            None => false,
        },
    }
}

/// Order two repository patterns by specificity, most specific first.
///
/// An exact `owner/repo` is more specific than an owner wildcard `org/*`,
/// which is more specific than the catch-all `*`.
pub fn most_specific(pattern_a: &str, pattern_b: &str) -> Ordering {
    specificity(pattern_a).cmp(&specificity(pattern_b))
}

fn specificity(pattern: &str) -> u8 {
    match pattern {
        "*" => 2,
        _ => match pattern.split_once('/') {
            Some((_, "*")) => 1,
            Some(_) => 0,
            None => 3,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_wildcard_matches_by_owner_only() {
        assert!(repo_matches("org/*", "org", "x"));
        assert!(repo_matches("org/*", "org", "any-repo"));
        assert!(!repo_matches("org/*", "other", "x"));
        assert!(!repo_matches("org/*", "org", ""));
    }

    #[test]
    fn exact_pattern_matches_owner_and_repo() {
        assert!(repo_matches("owner/repo", "owner", "repo"));
        assert!(!repo_matches("owner/repo", "owner", "other"));
        assert!(!repo_matches("owner/repo", "other", "repo"));
    }

    #[test]
    fn catch_all_matches_every_repository() {
        assert!(repo_matches("*", "any", "r"));
        assert!(repo_matches("*", "someone", "somewhere"));
    }

    #[test]
    fn most_specific_orders_exact_first() {
        assert_eq!(most_specific("org/repo", "org/*"), Ordering::Less);
        assert_eq!(most_specific("org/*", "*"), Ordering::Less);
        assert_eq!(most_specific("*", "org/*"), Ordering::Greater);
        assert_eq!(most_specific("org/repo", "org/repo"), Ordering::Equal);
        assert_eq!(most_specific("a/repo", "b/repo"), Ordering::Equal);
    }

    #[test]
    fn scope_from_name_maps_canonical_names() {
        assert_eq!(Scope::from_name("contents"), Some(Scope::Contents));
        assert_eq!(Scope::from_name("pull_requests"), Some(Scope::PullRequests));
        assert_eq!(Scope::from_name("issues"), Some(Scope::Issues));
        assert_eq!(Scope::from_name("actions"), Some(Scope::Actions));
        assert_eq!(Scope::from_name("metadata"), Some(Scope::Metadata));
        assert_eq!(Scope::from_name("packages"), None);
    }
}
