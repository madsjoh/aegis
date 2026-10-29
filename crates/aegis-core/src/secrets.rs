//! Secrets management.
//!
//! Reads a sops encrypted secrets file, decrypts it in memory, and returns a
//! typed [`SecretsFile`]. The decrypted plaintext is never written to disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use serde::Deserialize;

/// A decrypted secrets file, keyed by integration name and then credential name.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct SecretsFile {
    #[serde(flatten)]
    pub integrations: BTreeMap<String, BTreeMap<String, Credential>>,
}

impl std::fmt::Debug for SecretsFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretsFile")
            .field("integrations", &self.integrations)
            .finish()
    }
}

/// A single named credential for one integration.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Credential {
    pub pat: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub repositories: Vec<RepoRule>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("pat", &self.pat.as_ref().map(|_| "[REDACTED]"))
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("repositories", &self.repositories)
            .finish()
    }
}

/// A repository pattern and the permissions it grants.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RepoRule {
    pub pattern: String,
    pub permissions: BTreeMap<String, Permission>,
}

/// A per scope permission, granted or denied by presence or absence in a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read,
    Write,
}

/// Where the age key that decrypts the secrets file lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgeSource {
    File(PathBuf),
    Keychain { service: String, account: String },
}

/// An error that can occur while decrypting a secrets file.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("failed to read age key file: {0}")]
    ReadAgeKey(#[source] std::io::Error),
    #[error("failed to run sops: {0}")]
    RunSops(#[source] std::io::Error),
    #[error("sops exited with {status}: {stderr}")]
    Sops { status: ExitStatus, stderr: String },
    #[error("failed to parse sops output as JSON: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("failed to run security find-generic-password: {0}")]
    KeychainSpawn(#[source] std::io::Error),
    #[error("security find-generic-password exited with {status}: {stderr}")]
    KeychainStatus { status: ExitStatus, stderr: String },
    #[error("age key is not valid UTF-8")]
    AgeKeyNotUtf8,
}

/// Decrypt a sops encrypted secrets file and parse it into a [`SecretsFile`].
///
/// The age key is passed to the `sops` subprocess through the `SOPS_AGE_KEY`
/// environment variable and never printed. The plaintext lives only in memory.
pub fn decrypt(secrets_path: &Path, age: &AgeSource) -> Result<SecretsFile, SecretsError> {
    let age_key = match age {
        AgeSource::File(path) => std::fs::read_to_string(path).map_err(SecretsError::ReadAgeKey)?,
        AgeSource::Keychain { service, account } => {
            let output = Command::new("security")
                .args(["find-generic-password", "-s", service, "-a", account, "-w"])
                .output()
                .map_err(SecretsError::KeychainSpawn)?;
            if !output.status.success() {
                return Err(SecretsError::KeychainStatus {
                    status: output.status,
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            String::from_utf8(output.stdout)
                .map_err(|_| SecretsError::AgeKeyNotUtf8)?
                .trim_end_matches(['\n', '\r'])
                .to_string()
        }
    };

    let output = Command::new("sops")
        .args(["--decrypt", "--input-type", "yaml", "--output-type", "json"])
        .arg(secrets_path)
        .env("SOPS_AGE_KEY", age_key)
        .output()
        .map_err(SecretsError::RunSops)?;

    if !output.status.success() {
        return Err(SecretsError::Sops {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    serde_json::from_slice(&output.stdout).map_err(SecretsError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn decrypt_reads_github_pat() {
        let secrets = decrypt(
            &fixture("secrets.yaml"),
            &AgeSource::File(fixture("test-key.txt")),
        )
        .expect("decrypt should succeed");

        let github = secrets
            .integrations
            .get("github")
            .expect("github integration present");
        let personal = github.get("personal").expect("personal credential present");

        assert_eq!(personal.pat.as_deref(), Some("github_pat_test_sentinel"));

        let anthropic = secrets
            .integrations
            .get("anthropic")
            .expect("anthropic integration present");
        let default = anthropic
            .get("default")
            .expect("default credential present");
        assert_eq!(default.api_key.as_deref(), Some("sk-ant-test-sentinel"));

        let openai = secrets
            .integrations
            .get("openai")
            .expect("openai integration present");
        let openai_default = openai.get("default").expect("default credential present");
        assert!(openai_default.repositories.is_empty());

        let rule = &personal.repositories[0];
        assert_eq!(rule.pattern, "owner/repo");
        assert_eq!(rule.permissions["contents"], Permission::Write);
        assert_eq!(rule.permissions["issues"], Permission::Read);
    }

    #[test]
    fn permission_deserializes_lowercase() {
        let read: Permission = serde_json::from_str("\"read\"").expect("read parses");
        let write: Permission = serde_json::from_str("\"write\"").expect("write parses");
        assert_eq!(read, Permission::Read);
        assert_eq!(write, Permission::Write);
    }

    #[test]
    fn debug_redacts_credential_secrets() {
        let credential = Credential {
            pat: Some("super_secret_pat".to_string()),
            api_key: Some("super_secret_api_key".to_string()),
            repositories: vec![],
            enabled: None,
        };
        let secrets = SecretsFile {
            integrations: BTreeMap::from([(
                "github".to_string(),
                BTreeMap::from([("personal".to_string(), credential.clone())]),
            )]),
        };
        let credential_debug = format!("{credential:?}");
        assert!(credential_debug.contains("[REDACTED]"));
        assert!(!credential_debug.contains("super_secret_pat"));
        assert!(!credential_debug.contains("super_secret_api_key"));

        let secrets_debug = format!("{secrets:?}");
        assert!(secrets_debug.contains("[REDACTED]"));
        assert!(!secrets_debug.contains("super_secret_pat"));
        assert!(!secrets_debug.contains("super_secret_api_key"));
    }
}
