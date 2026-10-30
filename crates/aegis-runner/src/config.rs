//! User configuration: scaffold and read.

use std::path::{Path, PathBuf};

use serde::Deserialize;

const VM_JSON: &str = "vm.json";
const SKILLS_JSON: &str = "skills.json";
const OPENCODE_JSONC: &str = "opencode.jsonc";

const DEFAULT_VM_JSON: &str = "{\"cpu\":4,\"mem\":4096}\n";
const DEFAULT_SKILLS_JSON: &str = "{\"anthropic\":false,\"mattpocock\":false,\"vercel\":false}\n";
const DEFAULT_OPENCODE_JSONC: &str = "{}\n";

/// VM resources, read from `vm.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VmConfig {
    #[serde(default = "default_cpu")]
    pub cpu: u32,
    #[serde(default = "default_mem")]
    pub mem: u32,
}

fn default_cpu() -> u32 {
    4
}

fn default_mem() -> u32 {
    4096
}

/// Metis leaf skill toggles, read from `skills.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)]
    pub anthropic: bool,
    #[serde(default)]
    pub mattpocock: bool,
    #[serde(default)]
    pub vercel: bool,
}

/// An error while reading the user configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// Create the configuration files with their defaults when they are missing.
/// Existing files are left untouched.
pub fn scaffold(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_if_missing(&dir.join(VM_JSON), DEFAULT_VM_JSON)?;
    write_if_missing(&dir.join(SKILLS_JSON), DEFAULT_SKILLS_JSON)?;
    write_if_missing(&dir.join(OPENCODE_JSONC), DEFAULT_OPENCODE_JSONC)?;
    Ok(())
}

fn write_if_missing(path: &Path, content: &str) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::write(path, content)?;
    }
    Ok(())
}

/// Read `vm.json`, falling back to the defaults when the file is absent.
pub fn read_vm(dir: &Path) -> Result<VmConfig, ConfigError> {
    let path = dir.join(VM_JSON);
    match read_if_present(&path)? {
        None => Ok(VmConfig { cpu: 4, mem: 4096 }),
        Some(content) => {
            serde_json::from_str(&content).map_err(|source| ConfigError::Parse { path, source })
        }
    }
}

/// Read `skills.json`, falling back to all false when the file is absent.
pub fn read_skills(dir: &Path) -> Result<SkillsConfig, ConfigError> {
    let path = dir.join(SKILLS_JSON);
    match read_if_present(&path)? {
        None => Ok(SkillsConfig {
            anthropic: false,
            mattpocock: false,
            vercel: false,
        }),
        Some(content) => {
            serde_json::from_str(&content).map_err(|source| ConfigError::Parse { path, source })
        }
    }
}

/// Read the file contents, returning `None` when it does not exist.
fn read_if_present(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-config-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scaffold_writes_default_files() {
        let dir = temp_dir("scaffold-defaults");
        scaffold(&dir).unwrap();

        // The Bash runner writes each default with a trailing newline.
        assert_eq!(
            std::fs::read_to_string(dir.join("vm.json")).unwrap(),
            "{\"cpu\":4,\"mem\":4096}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("skills.json")).unwrap(),
            "{\"anthropic\":false,\"mattpocock\":false,\"vercel\":false}\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("opencode.jsonc")).unwrap(),
            "{}\n"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scaffold_leaves_existing_file_unchanged() {
        let dir = temp_dir("scaffold-existing");
        std::fs::write(dir.join("vm.json"), "{\"cpu\":8,\"mem\":8192}").unwrap();
        scaffold(&dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("vm.json")).unwrap(),
            "{\"cpu\":8,\"mem\":8192}"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_vm_uses_defaults_when_absent() {
        let dir = temp_dir("vm-defaults");
        assert_eq!(read_vm(&dir).unwrap(), VmConfig { cpu: 4, mem: 4096 });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_vm_reads_values() {
        let dir = temp_dir("vm-values");
        std::fs::write(dir.join("vm.json"), "{\"cpu\":6,\"mem\":8192}").unwrap();
        assert_eq!(read_vm(&dir).unwrap(), VmConfig { cpu: 6, mem: 8192 });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_skills_uses_defaults_when_absent() {
        let dir = temp_dir("skills-defaults");
        assert_eq!(
            read_skills(&dir).unwrap(),
            SkillsConfig {
                anthropic: false,
                mattpocock: false,
                vercel: false
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_skills_reads_values() {
        let dir = temp_dir("skills-values");
        std::fs::write(dir.join("skills.json"), "{\"anthropic\":true}").unwrap();
        assert_eq!(
            read_skills(&dir).unwrap(),
            SkillsConfig {
                anthropic: true,
                mattpocock: false,
                vercel: false
            }
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
