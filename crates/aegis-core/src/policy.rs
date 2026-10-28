//! Access policy.
//!
//! Defines the GitHub policy data model and the repository pattern matching
//! used to narrow a GitHub token's scope. A pattern is either an exact
//! `owner/repo`, an owner wildcard `org/*`, or the catch all `*`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::secrets::Permission;

/// The curated set of GitHub fine grained permissions the gateway enforces.
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

/// The outcome of enforcing a policy for a single request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

/// A compiled repository rule: a pattern plus the permissions it grants.
///
/// A scope missing from the permissions map is treated as `none`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRepoRule {
    pub pattern: String,
    pub permissions: BTreeMap<Scope, Permission>,
}

/// A per repository GitHub policy, an ordered list of compiled rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubPolicy {
    rules: Vec<CompiledRepoRule>,
}

impl GithubPolicy {
    /// Build a policy from already compiled rules.
    pub fn new(rules: Vec<CompiledRepoRule>) -> GithubPolicy {
        GithubPolicy { rules }
    }

    /// Decide whether a request is allowed under this policy.
    pub fn evaluate(&self, method: &str, path_and_query: &str) -> Decision {
        let request = match classify(method, path_and_query) {
            Classification::OutsideRepo => {
                return match access_for_method(method) {
                    Access::Read => Decision::Allow,
                    Access::Write => Decision::Deny,
                };
            }
            Classification::Unrecognized => return Decision::Deny,
            Classification::Repo(request) => request,
        };

        let Some(rule) = self.matching_rule(&request.owner, &request.repo) else {
            return Decision::Deny;
        };

        let permission = rule.permissions.get(&request.scope).copied().or_else(|| {
            if request.scope == Scope::Metadata {
                Some(Permission::Read)
            } else {
                None
            }
        });

        match permission {
            Some(Permission::Write) => Decision::Allow,
            Some(Permission::Read) => match request.access {
                Access::Read => Decision::Allow,
                Access::Write => Decision::Deny,
            },
            None => Decision::Deny,
        }
    }

    fn matching_rule(&self, owner: &str, repo: &str) -> Option<&CompiledRepoRule> {
        let mut best: Option<&CompiledRepoRule> = None;
        for rule in &self.rules {
            if !repo_matches(&rule.pattern, owner, repo) {
                continue;
            }
            let replace = match best {
                None => true,
                Some(current) => most_specific(&rule.pattern, &current.pattern) == Ordering::Less,
            };
            if replace {
                best = Some(rule);
            }
        }
        best
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
/// which is more specific than the catch all `*`.
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

/// Whether a request reads or writes, derived from the HTTP method or the git
/// smart HTTP service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

/// A parsed repository scoped request.
struct ClassifiedRequest {
    owner: String,
    repo: String,
    scope: Scope,
    access: Access,
}

/// The result of classifying a request path and query.
enum Classification {
    OutsideRepo,
    Repo(ClassifiedRequest),
    Unrecognized,
}

fn access_for_method(method: &str) -> Access {
    match method {
        "GET" | "HEAD" | "OPTIONS" => Access::Read,
        _ => Access::Write,
    }
}

fn query_service(query: &str) -> Option<&str> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("service="))
}

/// Map an HTTP method and path with optional query string to a classification.
fn classify(method: &str, path_and_query: &str) -> Classification {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    if segments.first() == Some(&"repos") && segments.len() >= 3 {
        let owner = segments[1].to_string();
        let repo = segments[2].to_string();
        let scope = match segments.get(3).copied() {
            None => Scope::Metadata,
            Some("pulls") => Scope::PullRequests,
            Some("issues") => Scope::Issues,
            Some("actions") => Scope::Actions,
            Some(
                "contents" | "commits" | "tags" | "releases" | "git" | "tarball" | "zipball",
            ) => Scope::Contents,
            Some(_) => return Classification::Unrecognized,
        };
        return Classification::Repo(ClassifiedRequest {
            owner,
            repo,
            scope,
            access: access_for_method(method),
        });
    }

    if segments.len() >= 2 {
        let owner = segments[0];
        let repo = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
        if !repo.is_empty() {
            let access = match &segments[2..] {
                ["git-upload-pack"] => Some(Access::Read),
                ["git-receive-pack"] => Some(Access::Write),
                ["info", "refs"] => match query_service(query) {
                    Some("git-upload-pack") => Some(Access::Read),
                    Some("git-receive-pack") => Some(Access::Write),
                    _ => return Classification::Unrecognized,
                },
                _ => None,
            };
            if let Some(access) = access {
                return Classification::Repo(ClassifiedRequest {
                    owner: owner.to_string(),
                    repo: repo.to_string(),
                    scope: Scope::Contents,
                    access,
                });
            }
        }
    }

    Classification::OutsideRepo
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

    fn policy_with(pattern: &str, permissions: &[(Scope, Permission)]) -> GithubPolicy {
        let mut map = BTreeMap::new();
        for (scope, permission) in permissions {
            map.insert(*scope, *permission);
        }
        GithubPolicy::new(vec![CompiledRepoRule {
            pattern: pattern.to_string(),
            permissions: map,
        }])
    }

    fn policy_with_rules(rules: &[(&str, &[(Scope, Permission)])]) -> GithubPolicy {
        GithubPolicy::new(
            rules
                .iter()
                .map(|(pattern, permissions)| CompiledRepoRule {
                    pattern: (*pattern).to_string(),
                    permissions: permissions
                        .iter()
                        .map(|(scope, permission)| (*scope, *permission))
                        .collect(),
                })
                .collect(),
        )
    }

    #[test]
    fn contents_read_is_allowed_under_contents_read() {
        let policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(
            policy.evaluate("GET", "/repos/org/repo/contents/x"),
            Decision::Allow
        );
    }

    #[test]
    fn issues_write_is_denied_under_issues_read() {
        let policy = policy_with("org/repo", &[(Scope::Issues, Permission::Read)]);
        assert_eq!(
            policy.evaluate("POST", "/repos/org/repo/issues"),
            Decision::Deny
        );
    }

    #[test]
    fn git_receive_pack_write_is_denied_under_contents_read() {
        let read_policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(
            read_policy.evaluate("POST", "/org/repo.git/git-receive-pack"),
            Decision::Deny
        );
        let write_policy = policy_with("org/repo", &[(Scope::Contents, Permission::Write)]);
        assert_eq!(
            write_policy.evaluate("POST", "/org/repo.git/git-receive-pack"),
            Decision::Allow
        );
    }

    #[test]
    fn git_upload_pack_without_dot_git_is_allowed_under_contents_read() {
        let policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(
            policy.evaluate("POST", "/org/repo/git-upload-pack"),
            Decision::Allow
        );
        assert_eq!(
            policy.evaluate("GET", "/org/repo/info/refs?service=git-upload-pack"),
            Decision::Allow
        );
    }

    #[test]
    fn git_info_refs_receive_pack_is_write_and_denied_under_read_only() {
        let read_policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(
            read_policy.evaluate("GET", "/org/repo.git/info/refs?service=git-receive-pack"),
            Decision::Deny
        );
        assert_eq!(
            read_policy.evaluate("GET", "/org/repo/info/refs?service=git-receive-pack"),
            Decision::Deny
        );
        let write_policy = policy_with("org/repo", &[(Scope::Contents, Permission::Write)]);
        assert_eq!(
            write_policy.evaluate("GET", "/org/repo.git/info/refs?service=git-receive-pack"),
            Decision::Allow
        );
    }

    #[test]
    fn outside_repo_read_is_allowed() {
        let policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(policy.evaluate("GET", "/user"), Decision::Allow);
    }

    #[test]
    fn outside_repo_write_is_denied() {
        let policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(policy.evaluate("POST", "/orgs/o/repos"), Decision::Deny);
    }

    #[test]
    fn metadata_read_is_implied_and_unmodeled_subpath_is_denied() {
        let policy = policy_with("org/repo", &[(Scope::Contents, Permission::Read)]);
        assert_eq!(policy.evaluate("GET", "/repos/org/repo"), Decision::Allow);
        assert_eq!(
            policy.evaluate("GET", "/repos/org/repo/collaborators"),
            Decision::Deny
        );
    }

    #[test]
    fn metadata_read_is_allowed_under_metadata_read() {
        let policy = policy_with("org/repo", &[(Scope::Metadata, Permission::Read)]);
        assert_eq!(policy.evaluate("GET", "/repos/org/repo"), Decision::Allow);
    }

    #[test]
    fn most_specific_rule_wins_in_evaluate() {
        let policy = policy_with_rules(&[
            ("org/*", &[(Scope::Contents, Permission::Read)]),
            ("org/repo", &[(Scope::Contents, Permission::Write)]),
        ]);
        assert_eq!(
            policy.evaluate("POST", "/repos/org/repo/contents/x"),
            Decision::Allow
        );
        assert_eq!(
            policy.evaluate("POST", "/repos/org/other/contents/x"),
            Decision::Deny
        );
    }
}
