//! Secrets management.
//!
//! Reads a sops encrypted secrets file, decrypts it in memory, and returns a
//! typed [`SecretsFile`]. The decrypted plaintext is never written to disk.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    pub copilot: bool,
    #[serde(default)]
    pub copilot_oauth_token: Option<String>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("pat", &self.pat.as_ref().map(|_| "[REDACTED]"))
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field(
                "copilot_oauth_token",
                &self.copilot_oauth_token.as_ref().map(|_| "[REDACTED]"),
            )
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
    #[error("failed to update encrypted secrets: {0}")]
    Update(#[source] std::io::Error),
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

fn age_key(age: &AgeSource) -> Result<String, SecretsError> {
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
    Ok(age_key)
}

/// Decrypt secrets in memory, passing the age key through `SOPS_AGE_KEY`.
pub fn decrypt(secrets_path: &Path, age: &AgeSource) -> Result<SecretsFile, SecretsError> {
    serde_json::from_value(decrypt_document(secrets_path, age)?).map_err(SecretsError::Parse)
}

fn decrypt_document(
    secrets_path: &Path,
    age: &AgeSource,
) -> Result<serde_json::Value, SecretsError> {
    let age_key = age_key(age)?;

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

/// Update the OAuth token without writing decrypted secrets to disk.
pub fn update_copilot_oauth(
    path: &Path,
    age: &AgeSource,
    credential: &str,
    token: &str,
) -> Result<(), SecretsError> {
    let original = std::fs::read(path).map_err(SecretsError::Update)?;
    let temporary = path.with_extension(format!(
        "aegis-{}-{}.yaml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(SecretsError::Update)?;
        file.write_all(&original).map_err(SecretsError::Update)?;
        let index = format!(
            "[\"github\"][{}][\"copilot_oauth_token\"]",
            serde_json::to_string(credential).unwrap()
        );
        let mut child = Command::new("sops")
            .args([
                "set",
                "--value-stdin",
                "--input-type",
                "yaml",
                "--output-type",
                "yaml",
            ])
            .arg(&temporary)
            .arg(index)
            .env("SOPS_AGE_KEY", age_key(age)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(SecretsError::RunSops)?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(token).unwrap())
            .map_err(SecretsError::Update)?;
        let output = child.wait_with_output().map_err(SecretsError::RunSops)?;
        if !output.status.success() {
            return Err(SecretsError::Sops {
                status: output.status,
                stderr: "SOPS could not save the OAuth credential".to_string(),
            });
        }
        let mut expected = decrypt_document(path, age)?;
        let credential_document = expected
            .get_mut("github")
            .and_then(|credentials| credentials.get_mut(credential))
            .and_then(|credential| credential.as_object_mut())
            .ok_or_else(|| {
                SecretsError::Update(std::io::Error::other("GitHub credential not found"))
            })?;
        credential_document.insert(
            "copilot_oauth_token".to_string(),
            serde_json::Value::String(token.to_string()),
        );
        let updated = decrypt_document(&temporary, age)?;
        let ciphertext = std::fs::read_to_string(&temporary).map_err(SecretsError::Update)?;
        if updated != expected || token.is_empty() || ciphertext.contains(token) {
            return Err(SecretsError::Update(std::io::Error::other(
                "OAuth update verification failed",
            )));
        }
        if std::fs::read(path).map_err(SecretsError::Update)? != original {
            return Err(SecretsError::Update(std::io::Error::other(
                "secrets changed during OAuth update; retry login",
            )));
        }
        std::fs::rename(&temporary, path).map_err(SecretsError::Update)
    })();
    let _ = std::fs::remove_file(temporary);
    result
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
    fn copilot_defaults_to_false() {
        let credential: Credential = serde_json::from_str(r#"{"pat":"sentinel"}"#).unwrap();
        assert!(!credential.copilot);
    }

    #[test]
    fn copilot_is_enabled_on_github_credential() {
        let credential: Credential =
            serde_json::from_str(r#"{"pat":"sentinel","copilot":true}"#).unwrap();
        assert!(credential.copilot);
    }

    #[test]
    fn oauth_update_preserves_existing_secrets_and_ciphertext() {
        let directory =
            std::env::temp_dir().join(format!("aegis-sops-update-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("secrets.yaml");
        std::fs::copy(fixture("secrets.yaml"), &path).unwrap();
        let age = AgeSource::File(fixture("test-key.txt"));
        let before = decrypt(&path, &age).unwrap();
        update_copilot_oauth(&path, &age, "personal", "oauth-sentinel").unwrap();
        let after = decrypt(&path, &age).unwrap();
        assert_eq!(
            after.integrations["github"]["personal"]
                .copilot_oauth_token
                .as_deref(),
            Some("oauth-sentinel")
        );
        assert_eq!(
            after.integrations["github"]["personal"].pat,
            before.integrations["github"]["personal"].pat
        );
        assert_eq!(
            after.integrations["github"]["personal"].repositories,
            before.integrations["github"]["personal"].repositories
        );
        assert_eq!(after.integrations["openai"], before.integrations["openai"]);
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("oauth-sentinel"));
        std::fs::remove_dir_all(directory).unwrap();
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
            copilot: false,
            copilot_oauth_token: Some("super_secret_oauth".to_string()),
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
        assert!(!credential_debug.contains("super_secret_oauth"));

        let secrets_debug = format!("{secrets:?}");
        assert!(secrets_debug.contains("[REDACTED]"));
        assert!(!secrets_debug.contains("super_secret_pat"));
        assert!(!secrets_debug.contains("super_secret_api_key"));
        assert!(!secrets_debug.contains("super_secret_oauth"));
    }
}
