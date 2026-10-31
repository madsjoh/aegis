//! Host SSH key management.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::lock::FileLock;

/// An error while managing the host SSH key.
#[derive(Debug, thiserror::Error)]
pub enum SshKeyError {
    #[error("failed to run ssh-keygen: {0}")]
    Run(#[source] std::io::Error),
    #[error("ssh-keygen failed to derive the public key: {stderr}")]
    Derive { stderr: String },
    #[error("an unreadable private key already exists at {0}")]
    InvalidExistingKey(String),
    #[error("failed to manage key files: {0}")]
    Io(#[source] std::io::Error),
}

/// Ensure an ed25519 host key pair exists at `path` (`path` and `path.pub`),
/// generating or repairing it as needed. Concurrent callers are serialized by a
/// lock on `path.lock`.
pub fn ensure_ssh_key(path: &Path) -> Result<(), SshKeyError> {
    let lock_path = with_suffix(path, "lock");
    let _lock = FileLock::acquire_blocking(&lock_path).map_err(SshKeyError::Io)?;

    match derive_public_key(path) {
        Ok(derived) => {
            if first_two_fields(&derived) == published_fields(path) {
                return Ok(());
            }
            write_pub_atomic(path, &derived)
        }
        Err(_) => {
            if path.exists() || path.is_symlink() {
                return Err(SshKeyError::InvalidExistingKey(path.display().to_string()));
            }
            generate(path)
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(format!(".{suffix}"));
    PathBuf::from(os)
}

fn derive_public_key(path: &Path) -> Result<String, SshKeyError> {
    let output = Command::new("ssh-keygen")
        .args(["-y", "-f"])
        .arg(path)
        .output()
        .map_err(SshKeyError::Run)?;
    if !output.status.success() {
        return Err(SshKeyError::Derive {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn first_two_fields(line: &str) -> String {
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(typ), Some(key)) => format!("{typ} {key}"),
        (Some(typ), None) => typ.to_string(),
        (None, _) => String::new(),
    }
}

fn published_fields(path: &Path) -> String {
    std::fs::read_to_string(with_suffix(path, "pub"))
        .map(|content| first_two_fields(&content))
        .unwrap_or_default()
}

fn write_pub_atomic(path: &Path, derived: &str) -> Result<(), SshKeyError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp_dir = create_temp_dir(parent)?;
    let temp_pub = temp_dir.join("key.pub");
    std::fs::write(&temp_pub, format!("{derived}\n")).map_err(SshKeyError::Io)?;
    std::fs::rename(&temp_pub, with_suffix(path, "pub")).map_err(SshKeyError::Io)?;
    let _ = std::fs::remove_dir(&temp_dir);
    Ok(())
}

fn create_temp_dir(parent: &Path) -> Result<PathBuf, SshKeyError> {
    let mut bytes = [0u8; 16];
    let mut urandom = std::fs::File::open("/dev/urandom").map_err(SshKeyError::Io)?;
    urandom.read_exact(&mut bytes).map_err(SshKeyError::Io)?;
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let dir = parent.join(format!(".aegis-key-{suffix}"));
    std::fs::create_dir(&dir).map_err(SshKeyError::Io)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(SshKeyError::Io)?;
    Ok(dir)
}

fn generate(path: &Path) -> Result<(), SshKeyError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temp_dir = create_temp_dir(parent)?;
    let temp_key = temp_dir.join("key");

    let status = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-f"])
        .arg(&temp_key)
        .args(["-N", "", "-q"])
        .status()
        .map_err(SshKeyError::Run)?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(SshKeyError::Derive {
            stderr: "ssh-keygen -t ed25519 failed".to_string(),
        });
    }

    let _ = std::fs::remove_file(with_suffix(&temp_key, "pub"));
    std::fs::rename(&temp_key, path).map_err(|error| {
        let _ = std::fs::remove_dir_all(&temp_dir);
        SshKeyError::Io(error)
    })?;

    let derived = match derive_public_key(path) {
        Ok(derived) => derived,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(error);
        }
    };

    std::fs::write(temp_dir.join("key.pub"), format!("{derived}\n")).map_err(SshKeyError::Io)?;
    std::fs::rename(temp_dir.join("key.pub"), with_suffix(path, "pub")).map_err(SshKeyError::Io)?;
    let _ = std::fs::remove_dir(&temp_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn temp_key(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-key-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("ssh_host_ed25519")
    }

    fn pub_of(path: &Path) -> PathBuf {
        let mut os = path.as_os_str().to_owned();
        os.push(".pub");
        PathBuf::from(os)
    }

    fn derived_public(path: &Path) -> String {
        let output = Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success(), "ssh-keygen -y must succeed");
        String::from_utf8(output.stdout).unwrap()
    }

    fn fields(line: &str) -> String {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(typ), Some(key)) => format!("{typ} {key}"),
            (Some(typ), None) => typ.to_string(),
            (None, _) => String::new(),
        }
    }

    #[test]
    fn generate_creates_a_matching_pair() {
        let path = temp_key("generate");
        assert!(!path.exists());

        ensure_ssh_key(&path).unwrap();

        assert!(path.exists());
        assert!(pub_of(&path).exists());
        assert_eq!(
            fields(&derived_public(&path)),
            fields(&std::fs::read_to_string(pub_of(&path)).unwrap())
        );
    }

    #[test]
    fn missing_public_key_is_repaired_without_replacing_the_private_key() {
        let path = temp_key("repair-missing");
        ensure_ssh_key(&path).unwrap();
        let private_before = derived_public(&path);

        std::fs::remove_file(pub_of(&path)).unwrap();
        ensure_ssh_key(&path).unwrap();

        assert_eq!(derived_public(&path), private_before);
        assert_eq!(
            fields(&derived_public(&path)),
            fields(&std::fs::read_to_string(pub_of(&path)).unwrap())
        );
    }

    #[test]
    fn mismatched_public_key_is_repaired_without_replacing_the_private_key() {
        let path = temp_key("repair-mismatch");
        ensure_ssh_key(&path).unwrap();
        let private_before = derived_public(&path);

        std::fs::write(pub_of(&path), "ssh-ed25519 AAAAwrong sentinel\n").unwrap();
        ensure_ssh_key(&path).unwrap();

        assert_eq!(derived_public(&path), private_before);
        assert_eq!(
            fields(&derived_public(&path)),
            fields(&std::fs::read_to_string(pub_of(&path)).unwrap())
        );
    }

    #[test]
    fn invalid_private_key_fails_without_changing_either_file() {
        let path = temp_key("invalid");
        std::fs::write(&path, "not a private key").unwrap();
        std::fs::write(pub_of(&path), "sentinel public key").unwrap();

        let key_before = std::fs::read(&path).unwrap();
        let pub_before = std::fs::read(pub_of(&path)).unwrap();

        assert!(ensure_ssh_key(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), key_before);
        assert_eq!(std::fs::read(pub_of(&path)).unwrap(), pub_before);
    }

    #[test]
    fn dangling_private_key_link_fails_without_changing_either_path() {
        let path = temp_key("dangling");
        let target = temp_key("missing-target");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        std::fs::write(pub_of(&path), "sentinel public key").unwrap();

        let pub_before = std::fs::read(pub_of(&path)).unwrap();

        assert!(ensure_ssh_key(&path).is_err());
        assert!(path.is_symlink());
        assert_eq!(std::fs::read(pub_of(&path)).unwrap(), pub_before);
    }

    #[test]
    fn concurrent_first_use_yields_a_matching_pair() {
        let path = temp_key("concurrent");
        let (tx, rx) = std::sync::mpsc::channel();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                ensure_ssh_key(&path).unwrap();
                tx.send(()).unwrap();
            }));
        }
        for _ in 0..2 {
            rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        }
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            fields(&derived_public(&path)),
            fields(&std::fs::read_to_string(pub_of(&path)).unwrap())
        );
    }

    #[test]
    fn temp_dir_name_is_a_random_hex_string() {
        let parent = std::env::temp_dir().join(format!(
            "aegis-key-parent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&parent).unwrap();

        let dir = create_temp_dir(&parent).unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let suffix = name.strip_prefix(".aegis-key-").expect("temp dir prefix");
        assert!(!suffix.is_empty());
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "temp dir suffix should be a random hex string, got {suffix:?}"
        );

        let _ = std::fs::remove_dir(&dir);
        let _ = std::fs::remove_dir(&parent);
    }
}
