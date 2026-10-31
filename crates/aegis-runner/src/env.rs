//! Build environment resolution.

use std::path::PathBuf;

use crate::config::{SkillsConfig, VmConfig};
use crate::paths::Paths;

/// Everything the runner shares with the guest VM build.
pub struct Runtime {
    pub workspace: PathBuf,
    pub paths: Paths,
    pub vm: VmConfig,
    pub skills: SkillsConfig,
    pub host_uid: u32,
    pub host_gid: u32,
    pub gateway_enabled: bool,
    pub proxy_port: u16,
}

/// The environment variables exported to the VM build, in order.
pub fn build_environment(runtime: &Runtime) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOST_WORKSPACE".to_string(), path_string(&runtime.workspace)),
        ("HOST_CONFIG".to_string(), path_string(&runtime.paths.config_dir)),
        (
            "HOST_STATE_DIR".to_string(),
            path_string(&runtime.paths.state_dir),
        ),
        ("VM_MOUNT_TAG".to_string(), runtime.paths.mount_tag.clone()),
        ("VM_CID".to_string(), runtime.paths.cid.to_string()),
        ("VM_SSH_PORT".to_string(), runtime.paths.ssh_port.to_string()),
        ("VM_HOST_UID".to_string(), runtime.host_uid.to_string()),
        ("VM_HOST_GID".to_string(), runtime.host_gid.to_string()),
        ("VM_CPU".to_string(), runtime.vm.cpu.to_string()),
        ("VM_MEM".to_string(), runtime.vm.mem.to_string()),
        (
            "VM_SKILL_ANTHROPIC".to_string(),
            runtime.skills.anthropic.to_string(),
        ),
        (
            "VM_SKILL_MATTPOCOCK".to_string(),
            runtime.skills.mattpocock.to_string(),
        ),
        (
            "VM_SKILL_VERCEL".to_string(),
            runtime.skills.vercel.to_string(),
        ),
    ];

    if runtime.gateway_enabled {
        env.push(("VM_PROXY_PORT".to_string(), runtime.proxy_port.to_string()));
    }

    env
}

fn path_string(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SkillsConfig, VmConfig};
    use crate::paths::{resolve, BaseDirs};
    use std::path::PathBuf;

    fn runtime(gateway_enabled: bool) -> Runtime {
        let base = BaseDirs {
            config: "/tmp/cfg".into(),
            data: "/tmp/data".into(),
            cache: "/tmp/cache".into(),
            state: "/tmp/state".into(),
        };
        let workspace = PathBuf::from("/tmp/workspace");
        let paths = resolve(&workspace, &base);
        Runtime {
            workspace,
            paths,
            vm: VmConfig { cpu: 6, mem: 8192 },
            skills: SkillsConfig {
                anthropic: true,
                mattpocock: false,
                vercel: false,
            },
            host_uid: 1000,
            host_gid: 100,
            gateway_enabled,
            proxy_port: 3128,
        }
    }

    fn env_value(env: &[(String, String)], key: &str) -> Option<String> {
        env.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    }

    #[test]
    fn always_emits_base_variables() {
        let rt = runtime(false);
        let env = build_environment(&rt);

        assert_eq!(env_value(&env, "HOST_WORKSPACE"), Some("/tmp/workspace".to_string()));
        assert_eq!(env_value(&env, "HOST_CONFIG"), Some("/tmp/cfg/aegis".to_string()));
        assert_eq!(
            env_value(&env, "HOST_STATE_DIR"),
            Some(rt.paths.state_dir.to_string_lossy().into_owned())
        );
        assert_eq!(env_value(&env, "VM_MOUNT_TAG"), Some(rt.paths.mount_tag.clone()));
        assert_eq!(env_value(&env, "VM_CID"), Some(rt.paths.cid.to_string()));
        assert_eq!(env_value(&env, "VM_SSH_PORT"), Some(rt.paths.ssh_port.to_string()));
        assert_eq!(env_value(&env, "VM_HOST_UID"), Some("1000".to_string()));
        assert_eq!(env_value(&env, "VM_HOST_GID"), Some("100".to_string()));
        assert_eq!(env_value(&env, "VM_CPU"), Some("6".to_string()));
        assert_eq!(env_value(&env, "VM_MEM"), Some("8192".to_string()));
        assert_eq!(env_value(&env, "VM_SKILL_ANTHROPIC"), Some("true".to_string()));
        assert_eq!(env_value(&env, "VM_SKILL_MATTPOCOCK"), Some("false".to_string()));
        assert_eq!(env_value(&env, "VM_SKILL_VERCEL"), Some("false".to_string()));
    }

    #[test]
    fn omits_proxy_port_when_gateway_disabled() {
        let env = build_environment(&runtime(false));
        assert!(env_value(&env, "VM_PROXY_PORT").is_none());
    }

    #[test]
    fn emits_proxy_port_when_gateway_enabled() {
        let env = build_environment(&runtime(true));
        assert_eq!(env_value(&env, "VM_PROXY_PORT"), Some("3128".to_string()));
    }
}
