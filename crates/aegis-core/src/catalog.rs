//! Integration catalog.
//!
//! Defines the built in integrations the gateway proxies, including the hosts
//! they serve and the header and placeholder used to substitute credentials.

use std::collections::BTreeMap;

/// One built in integration that the gateway can proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationSpec {
    pub hosts: Vec<String>,
    pub auth_header: &'static str,
    pub placeholder: &'static str,
}

/// Return the built in integration catalog keyed by integration name.
pub fn catalog() -> BTreeMap<String, IntegrationSpec> {
    BTreeMap::from([
        (
            "anthropic".to_string(),
            IntegrationSpec {
                hosts: vec!["api.anthropic.com".to_string()],
                auth_header: "x-api-key",
                placeholder: "sk-ant-aegis-placeholder",
            },
        ),
        (
            "copilot".to_string(),
            IntegrationSpec {
                hosts: vec!["api.githubcopilot.com".to_string()],
                auth_header: "authorization",
                placeholder: "copilot-aegis-placeholder",
            },
        ),
        (
            "deepseek".to_string(),
            IntegrationSpec {
                hosts: vec!["api.deepseek.com".to_string()],
                auth_header: "authorization",
                placeholder: "sk-aegis-deepseek-placeholder",
            },
        ),
        (
            "github".to_string(),
            IntegrationSpec {
                hosts: vec![
                    "github.com".to_string(),
                    "api.github.com".to_string(),
                    "*.github.com".to_string(),
                    "*.githubusercontent.com".to_string(),
                ],
                auth_header: "authorization",
                placeholder: "github_pat_aegis_placeholder",
            },
        ),
        (
            "google".to_string(),
            IntegrationSpec {
                hosts: vec!["generativelanguage.googleapis.com".to_string()],
                auth_header: "x-goog-api-key",
                placeholder: "AQ.aegis-placeholder",
            },
        ),
        (
            "openai".to_string(),
            IntegrationSpec {
                hosts: vec!["api.openai.com".to_string()],
                auth_header: "authorization",
                placeholder: "sk-aegis-placeholder",
            },
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::catalog;

    #[test]
    fn pinned_placeholders_and_headers() {
        assert_eq!(
            catalog()["github"].placeholder,
            "github_pat_aegis_placeholder"
        );
        assert_eq!(catalog()["anthropic"].auth_header, "x-api-key");
        assert_eq!(catalog()["google"].auth_header, "x-goog-api-key");
        assert_eq!(
            catalog()["google"].placeholder,
            "AQ.aegis-placeholder"
        );
        assert_eq!(
            catalog()["google"].hosts,
            vec!["generativelanguage.googleapis.com".to_string()]
        );
        assert_eq!(catalog()["deepseek"].auth_header, "authorization");
        assert_eq!(
            catalog()["deepseek"].placeholder,
            "sk-aegis-deepseek-placeholder"
        );
    }
}
