//! Integration test: the runner resolves and shares user configuration, then
//! stops at the VM build boundary through a fake `nix`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "aegis-runner-config-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_nix_dir(capture: &Path) -> PathBuf {
    let dir = temp_dir("fake-nix");
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" != \"build\" ]; then exit 2; fi\n\
         printf '%s\\n' \"$HOST_WORKSPACE\" \"$HOST_CONFIG\" \"$VM_CPU\" \"$VM_MEM\" \"$VM_SKILL_ANTHROPIC\" \"$VM_SKILL_MATTPOCOCK\" \"$VM_SKILL_VERCEL\" > \"{}\"\n\
         exit 1\n",
        capture.display()
    );
    std::fs::write(dir.join("nix"), script).unwrap();
    std::fs::set_permissions(
        dir.join("nix"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    dir
}

struct Environment {
    home: PathBuf,
    cache: PathBuf,
    data: PathBuf,
    state: PathBuf,
    workspace: PathBuf,
    config_home: Option<PathBuf>,
}

impl Environment {
    fn config_dir(&self) -> PathBuf {
        match &self.config_home {
            Some(path) => path.join("aegis"),
            None => self.home.join(".config/aegis"),
        }
    }
}

struct RunResult {
    status: i32,
    output: String,
}

fn run_runner(env: &Environment, nix_dir: &Path) -> RunResult {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aegis"));
    cmd.current_dir(&env.workspace);
    cmd.env("HOME", &env.home);
    cmd.env("XDG_CACHE_HOME", &env.cache);
    cmd.env("XDG_DATA_HOME", &env.data);
    cmd.env("XDG_STATE_HOME", &env.state);
    for name in ["HOST_CONFIG", "HOST_STATE_DIR", "HOST_WORKSPACE"] {
        cmd.env_remove(name);
    }
    match &env.config_home {
        Some(path) => {
            cmd.env("XDG_CONFIG_HOME", path);
        }
        None => {
            cmd.env_remove("XDG_CONFIG_HOME");
        }
    }
    let original_path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{original_path}", nix_dir.display()));
    let output = cmd.output().unwrap();
    RunResult {
        status: output.status.code().unwrap_or(-1),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

fn new_environment(config_home: Option<PathBuf>) -> Environment {
    let root = temp_dir("scenario");
    let env = Environment {
        home: root.join("home"),
        cache: root.join("cache"),
        data: root.join("data"),
        state: root.join("state"),
        workspace: root.join("workspace"),
        config_home,
    };
    std::fs::create_dir_all(env.config_dir()).unwrap();
    std::fs::create_dir_all(&env.workspace).unwrap();
    std::fs::create_dir_all(&env.home).unwrap();
    env
}

#[test]
fn runner_resolves_and_shares_user_configuration() {
    let env = new_environment(None);
    std::fs::write(env.config_dir().join("vm.json"), "{\"cpu\":6,\"mem\":8192}").unwrap();
    std::fs::write(env.config_dir().join("skills.json"), "{\"anthropic\":true}").unwrap();

    let capture = env.home.join("capture");
    let nix_dir = fake_nix_dir(&capture);

    let result = run_runner(&env, &nix_dir);

    assert_eq!(result.status, 1, "runner must exit with the nix build status");
    let expected = format!(
        "{}\n{}\n6\n8192\ntrue\nfalse\nfalse\n",
        std::fs::canonicalize(&env.workspace).unwrap().display(),
        env.config_dir().display()
    );
    assert_eq!(std::fs::read_to_string(&capture).unwrap(), expected);
    assert!(
        result.output.contains(&format!("Config    : {}", env.config_dir().display())),
        "runner must display the configuration directory, got: {}",
        result.output
    );

    for file in ["vm.json", "skills.json", "opencode.jsonc"] {
        assert!(env.config_dir().join(file).exists(), "runner must scaffold {file}");
    }
}

#[test]
fn runner_authorizes_copilot_before_provisioning_credentials() {
    let environment = new_environment(None);
    std::fs::write(environment.config_dir().join("secrets.yaml"), "encrypted fixture\n").unwrap();
    let capture = environment.home.join("gateway-commands");
    let directory = fake_nix_dir(&capture);
    std::fs::create_dir_all(directory.join("bin")).unwrap();
    let gateway = directory.join("bin/aegis-gateway");
    std::fs::write(&gateway, format!(
        "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\nif [ \"$1\" = 'login-copilot' ]; then printf '%s\\n' 'Enter code: test-code' >&2; fi\n",
        capture.display()
    )).unwrap();
    std::fs::set_permissions(&gateway, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::fs::write(directory.join("nix"), format!(
        "#!/bin/sh\ncase \"$2\" in *'#aegis-gateway') printf '%s\\n' '{}' ;; *) exit 1 ;; esac\n",
        directory.display()
    )).unwrap();
    let result = run_runner(&environment, &directory);
    assert_eq!(result.status, 1);
    assert_eq!(std::fs::read_to_string(capture).unwrap(), "ensure-ca\nlogin-copilot\nprovision-auth\n");
    assert!(result.output.contains("Enter code: test-code"));
}

#[test]
fn unknown_command_reports_an_error() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aegis"));
    cmd.arg("init");
    let output = cmd.output().unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(combined.contains("unknown command 'init'"), "got: {combined}");
}

#[test]
fn xdg_config_home_with_a_space_is_honored() {
    let config_home = temp_dir("config-with-space").join("custom configuration");
    let env = new_environment(Some(config_home));
    std::fs::write(env.config_dir().join("vm.json"), "{\"cpu\":2,\"mem\":2048}").unwrap();

    let capture = env.home.join("capture");
    let nix_dir = fake_nix_dir(&capture);

    let result = run_runner(&env, &nix_dir);

    assert_eq!(result.status, 1);
    let expected = format!(
        "{}\n{}\n2\n2048\nfalse\nfalse\nfalse\n",
        std::fs::canonicalize(&env.workspace).unwrap().display(),
        env.config_dir().display()
    );
    assert_eq!(std::fs::read_to_string(&capture).unwrap(), expected);
    assert!(
        result.output.contains(&format!("Config    : {}", env.config_dir().display())),
        "got: {}",
        result.output
    );
}

#[test]
fn darwin_vm_launch_receives_shared_directory_environment() {
    if !option_env!("AEGIS_HOST_SYSTEM")
        .unwrap_or("x86_64-linux")
        .ends_with("-darwin")
    {
        return;
    }

    let configuration_home = temp_dir("launch").join("custom configuration");
    let environment = new_environment(Some(configuration_home));
    let capture = environment.home.join("launch-environment");
    let nix_directory = fake_nix_dir(&capture);
    let vm_directory = temp_dir("vm");
    std::fs::create_dir_all(vm_directory.join("bin")).unwrap();
    std::fs::write(
        nix_directory.join("nix"),
        format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", vm_directory.display()),
    )
    .unwrap();
    let launcher = vm_directory.join("bin/run-aegis-vm");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$HOST_WORKSPACE\" \"$HOST_CONFIG\" \"$HOST_STATE_DIR\" > '{}'\nexit 1\n",
            capture.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &launcher,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let result = run_runner(&environment, &nix_directory);

    assert_eq!(result.status, 1);
    let actual = std::fs::read_to_string(&capture).unwrap_or_else(|error| {
        panic!("VM launch did not capture its environment: {error}; {}", result.output)
    });
    let values: Vec<_> = actual.lines().collect();
    assert_eq!(values.len(), 3);
    assert_eq!(
        values[0],
        environment.workspace.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(values[1], environment.config_dir().to_str().unwrap());
    assert!(Path::new(values[2]).starts_with(environment.state.join("aegis")));
    assert!(Path::new(values[2]).join("run/gitconfig/.gitconfig").is_file());
}
