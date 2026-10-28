//! Token table.
//!
//! Builds the set of proxy rules the gateway uses to match a request host and
//! decide how to rewrite the request.

use std::collections::BTreeMap;

use crate::catalog::catalog;
use crate::policy::{CompiledRepoRule, GithubPolicy, Scope};
use crate::secrets::{Permission, RepoRule, SecretsFile};

/// One proxy rule: the hosts it matches and how to rewrite matching requests.
#[derive(Clone, PartialEq, Eq)]
pub struct Rule {
    pub hosts: Vec<String>,
    pub auth_header: String,
    pub placeholder: String,
    pub secret: String,
    pub policy: Option<GithubPolicy>,
}

impl std::fmt::Debug for Rule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Rule")
            .field("hosts", &self.hosts)
            .field("auth_header", &self.auth_header)
            .field("placeholder", &self.placeholder)
            .field("secret", &"[REDACTED]")
            .field("policy", &self.policy)
            .finish()
    }
}

/// The token table: the ordered set of proxy rules built from the catalog.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenTable {
    rules: Vec<Rule>,
}

impl std::fmt::Debug for TokenTable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TokenTable")
            .field("rules", &self.rules)
            .finish()
    }
}

/// An error that can occur while building a token table.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("integration `{integration}` has no credential named `{credential}`")]
    MissingCredential {
        integration: String,
        credential: String,
    },
    #[error("credential `{credential}` for integration `{integration}` has no secret")]
    MissingSecret {
        integration: String,
        credential: String,
    },
}

impl TokenTable {
    /// Build a token table from an explicit rule list.
    pub fn new(rules: Vec<Rule>) -> TokenTable {
        TokenTable { rules }
    }

    /// Build a token table from the catalog, secrets, and a credential selection.
    pub fn build(secrets: &SecretsFile, selection: &Selection) -> Result<TokenTable, BuildError> {
        let mut rules = Vec::new();

        for (integration, spec) in catalog() {
            let Some(credential_name) = selection.credentials.get(&integration) else {
                continue;
            };
            let Some(credentials) = secrets.integrations.get(&integration) else {
                continue;
            };
            let Some(credential) = credentials.get(credential_name) else {
                return Err(BuildError::MissingCredential {
                    integration,
                    credential: credential_name.clone(),
                });
            };

            let secret = credential
                .pat
                .clone()
                .or_else(|| credential.api_key.clone())
                .ok_or_else(|| BuildError::MissingSecret {
                    integration: integration.clone(),
                    credential: credential_name.clone(),
                })?;

            let policy = if integration == "github" {
                Some(GithubPolicy::new(compile_repositories(
                    &credential.repositories,
                )))
            } else {
                None
            };

            rules.push(Rule {
                hosts: spec.hosts,
                auth_header: spec.auth_header.to_string(),
                placeholder: spec.placeholder.to_string(),
                secret,
                policy,
            });
        }

        Ok(TokenTable { rules })
    }

    /// Return the first rule whose host pattern matches the request host.
    pub fn matching_rule(&self, host: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|rule| rule.hosts.iter().any(|pattern| host_matches(pattern, host)))
    }
}

/// A credential selection: one credential name per integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub credentials: BTreeMap<String, String>,
}

impl Selection {
    /// Select the lexicographically first credential name for each integration.
    pub fn defaults(secrets: &SecretsFile) -> Selection {
        let credentials = secrets
            .integrations
            .iter()
            .filter_map(|(integration, credentials)| {
                credentials
                    .keys()
                    .next()
                    .map(|name| (integration.clone(), name.clone()))
            })
            .collect();

        Selection { credentials }
    }
}

/// Return whether a host pattern matches a host name.
///
/// A leading `*.` matches one or more dot separated labels before the suffix.
/// Any other pattern matches only itself exactly.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(suffix)
            .and_then(|rest| rest.strip_suffix('.'))
            .is_some_and(|prefix| !prefix.is_empty()),
        None => pattern == host,
    }
}

/// Compile repository rules into a policy rule, dropping unknown scope names
/// and implying `metadata: read` unless `metadata: write` is already present.
fn compile_repositories(repositories: &[RepoRule]) -> Vec<CompiledRepoRule> {
    repositories
        .iter()
        .map(|repository| {
            let mut permissions: BTreeMap<Scope, Permission> = repository
                .permissions
                .iter()
                .filter_map(|(name, permission)| {
                    Scope::from_name(name).map(|scope| (scope, *permission))
                })
                .collect();
            permissions
                .entry(Scope::Metadata)
                .or_insert(Permission::Read);

            CompiledRepoRule {
                pattern: repository.pattern.clone(),
                permissions,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::policy::Scope;
    use crate::secrets::{Credential, Permission, RepoRule, SecretsFile};

    fn secrets_with(integrations: BTreeMap<String, BTreeMap<String, Credential>>) -> SecretsFile {
        SecretsFile { integrations }
    }

    fn credential(
        pat: Option<&str>,
        api_key: Option<&str>,
        repositories: Vec<RepoRule>,
    ) -> Credential {
        Credential {
            pat: pat.map(|value| value.to_string()),
            api_key: api_key.map(|value| value.to_string()),
            repositories,
        }
    }

    #[test]
    fn wildcard_matches_subdomain_labels_but_not_bare_domain() {
        assert!(host_matches("*.github.com", "api.github.com"));
        assert!(host_matches("*.github.com", "raw.github.com"));
        assert!(!host_matches("*.github.com", "github.com"));
        assert!(!host_matches("*.github.com", "notgithub.com"));
    }

    #[test]
    fn exact_pattern_matches_only_itself() {
        assert!(host_matches("github.com", "github.com"));
        assert!(!host_matches("github.com", "api.github.com"));
        assert!(!host_matches("github.com", "github.com.evil"));
    }

    #[test]
    fn wildcard_matches_githubusercontent() {
        assert!(host_matches(
            "*.githubusercontent.com",
            "raw.githubusercontent.com"
        ));
        assert!(!host_matches(
            "*.githubusercontent.com",
            "githubusercontent.com"
        ));
    }

    #[test]
    fn defaults_select_lexicographically_first_credential() {
        let secrets = secrets_with(BTreeMap::from([
            (
                "github".to_string(),
                BTreeMap::from([
                    ("zeta".to_string(), credential(Some("z"), None, vec![])),
                    ("alpha".to_string(), credential(Some("a"), None, vec![])),
                ]),
            ),
            (
                "openai".to_string(),
                BTreeMap::from([("default".to_string(), credential(None, Some("k"), vec![]))]),
            ),
        ]));
        let selection = Selection::defaults(&secrets);
        assert_eq!(selection.credentials["github"], "alpha");
        assert_eq!(selection.credentials["openai"], "default");
    }

    #[test]
    fn build_makes_one_rule_per_selected_integration() {
        let secrets = secrets_with(BTreeMap::from([
            (
                "github".to_string(),
                BTreeMap::from([(
                    "personal".to_string(),
                    credential(Some("ghp"), None, vec![]),
                )]),
            ),
            (
                "openai".to_string(),
                BTreeMap::from([("default".to_string(), credential(None, Some("sk"), vec![]))]),
            ),
        ]));
        let selection = Selection::defaults(&secrets);
        let table = TokenTable::build(&secrets, &selection).expect("build succeeds");

        let github = table.matching_rule("api.github.com").expect("github rule");
        assert_eq!(github.secret, "ghp");
        assert_eq!(github.auth_header, "authorization");
        assert_eq!(github.placeholder, "github_pat_aegis_placeholder");
        assert!(github.policy.is_some());

        let openai = table.matching_rule("api.openai.com").expect("openai rule");
        assert_eq!(openai.secret, "sk");
        assert_eq!(openai.auth_header, "authorization");
        assert!(openai.policy.is_none());
    }

    #[test]
    fn build_skips_integration_absent_from_secrets() {
        let secrets = secrets_with(BTreeMap::from([(
            "github".to_string(),
            BTreeMap::from([(
                "personal".to_string(),
                credential(Some("ghp"), None, vec![]),
            )]),
        )]));
        let selection = Selection::defaults(&secrets);
        let table = TokenTable::build(&secrets, &selection).expect("build succeeds");
        assert!(table.matching_rule("api.anthropic.com").is_none());
        assert!(table.matching_rule("api.github.com").is_some());
    }

    #[test]
    fn build_errors_when_credential_has_no_secret() {
        let secrets = secrets_with(BTreeMap::from([(
            "github".to_string(),
            BTreeMap::from([("empty".to_string(), credential(None, None, vec![]))]),
        )]));
        let selection = Selection::defaults(&secrets);
        let result = TokenTable::build(&secrets, &selection);
        assert!(matches!(result, Err(BuildError::MissingSecret { .. })));
    }

    #[test]
    fn build_errors_when_selection_names_unknown_credential() {
        let secrets = secrets_with(BTreeMap::from([(
            "github".to_string(),
            BTreeMap::from([(
                "personal".to_string(),
                credential(Some("ghp"), None, vec![]),
            )]),
        )]));
        let selection = Selection {
            credentials: BTreeMap::from([("github".to_string(), "missing".to_string())]),
        };
        let result = TokenTable::build(&secrets, &selection);
        assert!(matches!(result, Err(BuildError::MissingCredential { .. })));
    }

    #[test]
    fn compile_repositories_maps_scopes_and_skips_unknown() {
        let repositories = vec![RepoRule {
            pattern: "owner/repo".to_string(),
            permissions: BTreeMap::from([
                ("contents".to_string(), Permission::Write),
                ("issues".to_string(), Permission::Read),
                ("unknown".to_string(), Permission::Read),
            ]),
        }];
        let compiled = compile_repositories(&repositories);
        assert_eq!(compiled.len(), 1);
        assert_eq!(compiled[0].pattern, "owner/repo");
        assert_eq!(compiled[0].permissions[&Scope::Contents], Permission::Write);
        assert_eq!(compiled[0].permissions[&Scope::Issues], Permission::Read);
        assert_eq!(compiled[0].permissions[&Scope::Metadata], Permission::Read);
        assert!(!compiled[0].permissions.contains_key(&Scope::PullRequests));
    }

    #[test]
    fn compile_repositories_implies_metadata_read_and_denies_collaborators() {
        use crate::policy::Decision;

        let repositories = vec![RepoRule {
            pattern: "org/repo".to_string(),
            permissions: BTreeMap::from([("contents".to_string(), Permission::Read)]),
        }];
        let policy = GithubPolicy::new(compile_repositories(&repositories));
        assert_eq!(policy.evaluate("GET", "/repos/org/repo"), Decision::Allow);
        assert_eq!(
            policy.evaluate("GET", "/repos/org/repo/collaborators"),
            Decision::Deny
        );
    }

    #[test]
    fn debug_redacts_rule_and_table_secrets() {
        let rule = Rule {
            hosts: vec!["api.github.com".to_string()],
            auth_header: "authorization".to_string(),
            placeholder: "github_pat_aegis_placeholder".to_string(),
            secret: "super_secret_token".to_string(),
            policy: None,
        };
        let table = TokenTable::new(vec![rule.clone()]);
        let rule_debug = format!("{rule:?}");
        assert!(rule_debug.contains("[REDACTED]"));
        assert!(!rule_debug.contains("super_secret_token"));

        let table_debug = format!("{table:?}");
        assert!(table_debug.contains("[REDACTED]"));
        assert!(!table_debug.contains("super_secret_token"));
    }
}
