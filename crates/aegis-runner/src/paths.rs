//! Path resolution for the Aegis runner.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The four host directories the runner derives from XDG and HOME.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseDirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
}

/// All runner paths and derived identifiers for one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub run_dir: PathBuf,
    pub opencode_state_dir: PathBuf,
    pub opencode_share_dir: PathBuf,
    pub workspace_id: String,
    pub mount_tag: String,
    pub cid: u32,
    pub ssh_port: u16,
    pub proxy_host_port: u16,
    pub ssh_key: PathBuf,
    pub workspace_lock_file: PathBuf,
    pub image_lock_file: PathBuf,
}

/// Resolve the four base directories, honoring XDG overrides and falling back
/// to the HOME directory.
pub fn base_dirs() -> BaseDirs {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();

    let resolve_dir = |name: &str, fallback: &Path| -> PathBuf {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| fallback.to_path_buf())
    };

    BaseDirs {
        config: resolve_dir("XDG_CONFIG_HOME", &home.join(".config")),
        data: resolve_dir("XDG_DATA_HOME", &home.join(".local/share")),
        cache: resolve_dir("XDG_CACHE_HOME", &home.join(".cache")),
        state: resolve_dir("XDG_STATE_HOME", &home.join(".local/state")),
    }
}

/// The workspace id: the first sixteen lowercase hex characters of the SHA-256
/// of the workspace path, matching the Bash runner.
pub fn workspace_id(workspace: &Path) -> String {
    let digest = Sha256::digest(workspace.to_string_lossy().as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Resolve the full set of runner paths and derived identifiers.
pub fn resolve(workspace: &Path, base: &BaseDirs) -> Paths {
    let id = workspace_id(workspace);
    let mount_tag = format!("ws_{}", &id[..8]);
    let cid = 3 + u32::from_str_radix(&id[..4], 16).unwrap_or(0) % 1000;

    let config_dir = base.config.join("aegis");
    let data_dir = base.data.join("aegis");
    let cache_dir = base.cache.join("aegis").join("vzvm");
    let state_dir = base.state.join("aegis").join(&id);
    let run_dir = state_dir.join("run");

    Paths {
        config_dir: config_dir.clone(),
        data_dir: data_dir.clone(),
        cache_dir: cache_dir.clone(),
        state_dir: state_dir.clone(),
        run_dir: run_dir.clone(),
        opencode_state_dir: state_dir.join("opencode").join("state"),
        opencode_share_dir: state_dir.join("opencode").join("share"),
        workspace_id: id,
        mount_tag,
        cid,
        ssh_port: (20000 + cid) as u16,
        proxy_host_port: (21000 + cid) as u16,
        ssh_key: data_dir.join("ssh_host_ed25519"),
        workspace_lock_file: run_dir.join("lock"),
        image_lock_file: cache_dir.join("lock"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn base() -> BaseDirs {
        BaseDirs {
            config: "/tmp/cfg".into(),
            data: "/tmp/data".into(),
            cache: "/tmp/cache".into(),
            state: "/tmp/state".into(),
        }
    }

    #[test]
    fn workspace_id_is_first_sixteen_sha256_hex_chars() {
        // `printf '%s' '/home/user/work' | sha256sum` starts with
        // ba58499e4e376a4b...
        assert_eq!(
            workspace_id(Path::new("/home/user/work")),
            "ba58499e4e376a4b"
        );
    }

    #[test]
    fn resolve_places_directories_under_base_dirs() {
        let paths = resolve(Path::new("/home/user/work"), &base());
        let id = workspace_id(Path::new("/home/user/work"));

        assert_eq!(paths.config_dir, Path::new("/tmp/cfg/aegis"));
        assert_eq!(paths.data_dir, Path::new("/tmp/data/aegis"));
        assert_eq!(paths.cache_dir, Path::new("/tmp/cache/aegis/vzvm"));
        assert_eq!(paths.state_dir, Path::new("/tmp/state/aegis").join(&id));
        assert_eq!(paths.run_dir, paths.state_dir.join("run"));
        assert_eq!(
            paths.opencode_state_dir,
            paths.state_dir.join("opencode/state")
        );
        assert_eq!(
            paths.opencode_share_dir,
            paths.state_dir.join("opencode/share")
        );
    }

    #[test]
    fn resolve_derives_mount_tag_and_ports() {
        let paths = resolve(Path::new("/home/user/work"), &base());
        let id = workspace_id(Path::new("/home/user/work"));
        let cid = 3 + u32::from_str_radix(&id[..4], 16).unwrap() % 1000;

        assert_eq!(paths.mount_tag, format!("ws_{}", &id[..8]));
        assert_eq!(paths.cid, cid);
        assert_eq!(paths.ssh_port, 20000 + cid as u16);
        assert_eq!(paths.proxy_host_port, 21000 + cid as u16);
        assert_eq!(paths.ssh_key, Path::new("/tmp/data/aegis/ssh_host_ed25519"));
        assert_eq!(paths.workspace_lock_file, paths.run_dir.join("lock"));
        assert_eq!(paths.image_lock_file, Path::new("/tmp/cache/aegis/vzvm/lock"));
    }

    #[test]
    fn resolve_is_deterministic() {
        let a = resolve(Path::new("/home/user/work"), &base());
        let b = resolve(Path::new("/home/user/work"), &base());

        assert_eq!(a.workspace_id, b.workspace_id);
        assert_eq!(a.mount_tag, b.mount_tag);
        assert_eq!(a.cid, b.cid);
        assert_eq!(a.ssh_port, b.ssh_port);
        assert_eq!(a.proxy_host_port, b.proxy_host_port);
    }

    #[test]
    fn base_dirs_honor_xdg_overrides() {
        let _guard = ENV_LOCK.lock().unwrap();

        std::env::set_var("HOME", "/tmp/home");
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/with space/config");
        std::env::set_var("XDG_DATA_HOME", "/tmp/data");
        std::env::set_var("XDG_CACHE_HOME", "/tmp/cache");
        std::env::set_var("XDG_STATE_HOME", "/tmp/state");

        let dirs = base_dirs();
        assert_eq!(dirs.config, Path::new("/tmp/with space/config"));
        assert_eq!(dirs.data, Path::new("/tmp/data"));
        assert_eq!(dirs.cache, Path::new("/tmp/cache"));
        assert_eq!(dirs.state, Path::new("/tmp/state"));
    }

    #[test]
    fn base_dirs_fall_back_to_home() {
        let _guard = ENV_LOCK.lock().unwrap();

        std::env::set_var("HOME", "/tmp/home");
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
        std::env::remove_var("XDG_CACHE_HOME");
        std::env::remove_var("XDG_STATE_HOME");

        let dirs = base_dirs();
        assert_eq!(dirs.config, Path::new("/tmp/home/.config"));
        assert_eq!(dirs.data, Path::new("/tmp/home/.local/share"));
        assert_eq!(dirs.cache, Path::new("/tmp/home/.cache"));
        assert_eq!(dirs.state, Path::new("/tmp/home/.local/state"));
    }
}
