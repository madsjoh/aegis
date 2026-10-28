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
    }
}
