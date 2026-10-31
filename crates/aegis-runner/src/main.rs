//! The Aegis runner: orchestrate the guest VM and gateway on the host.

mod config;
mod env;
mod lock;
mod paths;
mod process;
mod signal;
mod ssh_key;
mod store_cache;

use std::io::Read;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::Duration;

use lock::FileLock;
use process::ProcessError;

const USAGE: &str = "\
Usage: aegis [run [args...] | help]
  run   Boot the guest VM (default).
  help  Show this help.";

const FLAKE_REF: &str = match option_env!("AEGIS_FLAKE_REF") {
    Some(value) => value,
    None => "path:.",
};
const HOST_SYSTEM: &str = match option_env!("AEGIS_HOST_SYSTEM") {
    Some(value) => value,
    None => "x86_64-linux",
};
const GUEST_SYSTEM: &str = match option_env!("AEGIS_GUEST_SYSTEM") {
    Some(value) => value,
    None => "x86_64-linux",
};

const GUEST_PROXY_PORT: u16 = 3128;
const SSH_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None | Some("run") => {
            let forwarded: &[String] = args.get(2..).unwrap_or(&[]);
            run(forwarded)
        }
        Some("help") | Some("--help") | Some("-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("Error: unknown command '{other}'.");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

enum RunnerError {
    Command { message: String, code: i32 },
    Other(String),
    Interrupted(i32),
}

fn run(forwarded: &[String]) -> ExitCode {
    signal::install();
    match orchestrate(forwarded) {
        Ok(code) => code,
        Err(RunnerError::Command { message, code }) => {
            if !message.is_empty() {
                eprintln!("{message}");
            }
            ExitCode::from(code.clamp(0, 255) as u8)
        }
        Err(RunnerError::Interrupted(signal)) => signal::exit_code(signal),
        Err(RunnerError::Other(message)) => {
            eprintln!("Error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn is_darwin() -> bool {
    HOST_SYSTEM.ends_with("-darwin")
}

fn workspace_path() -> PathBuf {
    if let Some(pwd) = std::env::var_os("PWD") {
        let path = PathBuf::from(pwd);
        if path.is_absolute() {
            return path;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn host_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn host_gid() -> u32 {
    unsafe { libc::getgid() }
}

fn ssh_pub_path(key: &Path) -> PathBuf {
    let mut os = key.as_os_str().to_owned();
    os.push(".pub");
    PathBuf::from(os)
}

fn apply_env(command: &mut Command, env: &[(String, String)]) {
    for (key, value) in env {
        command.env(key, value);
    }
}

fn run_checked(mut command: Command) -> Result<std::process::Output, RunnerError> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| RunnerError::Other(error.to_string()))?;
    let status = loop {
        if let Some(signal) = signal::take() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunnerError::Interrupted(signal));
        }
        match child
            .try_wait()
            .map_err(|error| RunnerError::Other(error.to_string()))?
        {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let mut stdout = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout)
            .map_err(|error| RunnerError::Other(error.to_string()))?;
    }
    if !status.success() {
        return Err(RunnerError::Command {
            message: String::new(),
            code: status.code().unwrap_or(1),
        });
    }
    Ok(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn process_to_runner(error: ProcessError) -> RunnerError {
    match error {
        ProcessError::Interrupted(signal) => RunnerError::Interrupted(signal),
        other => RunnerError::Other(other.to_string()),
    }
}

fn guest_start_to_runner(error: store_cache::GuestStartError) -> RunnerError {
    match error {
        store_cache::GuestStartError::Interrupted(signal) => RunnerError::Interrupted(signal),
        other => RunnerError::Other(other.to_string()),
    }
}

fn nix_build(target: &str, env: &[(String, String)]) -> Result<String, RunnerError> {
    let mut command = Command::new("nix");
    command.args(["build", target, "--impure", "--no-link", "--print-out-paths"]);
    apply_env(&mut command, env);
    let output = run_checked(command)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn reg_info_basename(env: &[(String, String)]) -> Result<String, RunnerError> {
    let expression = format!(
        "let flake = builtins.getFlake \"{FLAKE_REF}\"; \
         pkgs = flake.inputs.nixpkgs.legacyPackages.{HOST_SYSTEM}; \
         vm = flake.nixosConfigurations.\"aegis-vm-{HOST_SYSTEM}\"; \
         regInfo = pkgs.closureInfo {{ rootPaths = vm.config.virtualisation.additionalPaths; }}; \
         in builtins.baseNameOf regInfo"
    );
    let mut command = Command::new("nix");
    command.args(["eval", "--raw", "--impure", "--expr", &expression]);
    apply_env(&mut command, env);
    let output = run_checked(command)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Background children the runner owns and tears down on exit.
struct Children {
    virtiofsd: Vec<Child>,
    gateway: Option<Child>,
    vm: Option<Child>,
    tunnel: Option<Child>,
}

impl Children {
    fn new() -> Self {
        Self {
            virtiofsd: Vec::new(),
            gateway: None,
            vm: None,
            tunnel: None,
        }
    }

    fn kill_all(&mut self) {
        for child in self.virtiofsd.iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.virtiofsd.clear();
        if let Some(child) = self.gateway.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.gateway = None;
        if let Some(child) = self.vm.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.vm = None;
        if let Some(child) = self.tunnel.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.tunnel = None;
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        self.kill_all();
    }
}

fn start_virtiofsd(
    socket: &Path,
    shared_dir: &Path,
    log: &Path,
    readonly: bool,
    host_uid: u32,
    host_gid: u32,
) -> Result<Child, ProcessError> {
    let mut command = Command::new("virtiofsd");
    command
        .arg("--socket-path")
        .arg(socket)
        .arg("--shared-dir")
        .arg(shared_dir)
        .arg("--thread-pool-size")
        .arg("4")
        .arg("--cache=auto")
        .arg("--translate-uid")
        .arg(format!("guest:1000:{host_uid}:1"))
        .arg("--translate-gid")
        .arg(format!("guest:100:{host_gid}:1"));
    if readonly {
        command.arg("--readonly");
    }
    process::spawn_logged(command, log)
}

fn socket_exists(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

fn dump_virtiofsd_logs(run_dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(run_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("virtiofsd-") && name.ends_with(".log") {
                process::dump_log(&entry.path());
            }
        }
    }
}

fn orchestrate(forwarded: &[String]) -> Result<ExitCode, RunnerError> {
    let base = paths::base_dirs();
    let workspace = workspace_path();
    let p = paths::resolve(&workspace, &base);

    for dir in [
        &p.data_dir,
        &p.cache_dir,
        &p.run_dir,
        &p.opencode_state_dir,
        &p.opencode_share_dir,
    ] {
        std::fs::create_dir_all(dir).map_err(|e| RunnerError::Other(e.to_string()))?;
    }

    config::scaffold(&p.config_dir).map_err(|e| RunnerError::Other(e.to_string()))?;

    let _workspace_lock = match FileLock::acquire(&p.workspace_lock_file)
        .map_err(|e| RunnerError::Other(e.to_string()))?
    {
        Some(lock) => lock,
        None => {
            return Err(RunnerError::Other(
                "An Aegis VM is already active in this workspace.".to_string(),
            ));
        }
    };

    let vm = config::read_vm(&p.config_dir).map_err(|e| RunnerError::Other(e.to_string()))?;
    let skills = config::read_skills(&p.config_dir).map_err(|e| RunnerError::Other(e.to_string()))?;

    ssh_key::ensure_ssh_key(&p.ssh_key).map_err(|e| RunnerError::Other(e.to_string()))?;
    let public_key = std::fs::read_to_string(ssh_pub_path(&p.ssh_key))
        .map_err(|e| RunnerError::Other(e.to_string()))?
        .trim_end()
        .to_string();
    let ssh_dir = p.config_dir.join("ssh");
    std::fs::create_dir_all(&ssh_dir).map_err(|e| RunnerError::Other(e.to_string()))?;
    std::fs::write(ssh_dir.join("authorized_keys"), format!("{public_key}\n"))
        .map_err(|e| RunnerError::Other(e.to_string()))?;

    let secrets_file = p.config_dir.join("secrets.yaml");
    let gateway_enabled = secrets_file.exists();
    let age_key_file = base.config.join("sops/age/keys.txt");
    let mut gateway_path = None;
    if gateway_enabled {
        println!("Provisioning the gateway CA...");
        let path = nix_build(&format!("{FLAKE_REF}#aegis-gateway"), &[])?;
        let gateway_bin = format!("{path}/bin/aegis-gateway");
        let mut command = Command::new(&gateway_bin);
        command
            .args(["ensure-ca", "--ca-dir"])
            .arg(p.data_dir.join("ca"))
            .args(["--cert-out"])
            .arg(p.config_dir.join("ca.crt"));
        run_checked(command)?;

        let mut command = Command::new(&gateway_bin);
        command
            .args(["provision-auth", "--secrets"])
            .arg(&secrets_file)
            .args(["--age-key"])
            .arg(&age_key_file)
            .args(["--out"])
            .arg(p.opencode_share_dir.join("credentials.json"));
        run_checked(command)?;
        gateway_path = Some(path);
    }

    let gitconfig_dir = p.run_dir.join("gitconfig");
    std::fs::create_dir_all(&gitconfig_dir).map_err(|e| RunnerError::Other(e.to_string()))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let home_gitconfig = home.join(".gitconfig");
    if home_gitconfig.is_file() {
        std::fs::copy(&home_gitconfig, gitconfig_dir.join(".gitconfig"))
            .map_err(|e| RunnerError::Other(e.to_string()))?;
    } else {
        std::fs::write(gitconfig_dir.join(".gitconfig"), "[user]\n")
            .map_err(|e| RunnerError::Other(e.to_string()))?;
    }

    let runtime = env::Runtime {
        workspace: workspace.clone(),
        paths: p.clone(),
        vm: vm.clone(),
        skills: skills.clone(),
        host_uid: host_uid(),
        host_gid: host_gid(),
        gateway_enabled,
        proxy_port: GUEST_PROXY_PORT,
    };
    let build_env = env::build_environment(&runtime);

    println!("Aegis [Host: {HOST_SYSTEM} | Guest: {GUEST_SYSTEM}]");
    println!("Workspace : {}", workspace.display());
    println!("Config    : {}", p.config_dir.display());
    println!("State     : {}", p.state_dir.display());

    println!("Building the guest VM...");
    let vm_path = nix_build(&format!("{FLAKE_REF}#aegis-vm-{HOST_SYSTEM}"), &build_env)?;
    let reg_info = if is_darwin() {
        None
    } else {
        Some(reg_info_basename(&build_env)?)
    };

    let mut children = Children::new();

    let mut virtiofsd_sockets = Vec::new();
    if !is_darwin() {
        println!("Starting the virtiofs daemons...");
        let specs = [
            (p.run_dir.join("workspace.sock"), workspace.clone(), false),
            (p.run_dir.join("config.sock"), p.config_dir.clone(), true),
            (
                p.run_dir.join("opencode-state.sock"),
                p.opencode_state_dir.clone(),
                false,
            ),
            (
                p.run_dir.join("opencode-share.sock"),
                p.opencode_share_dir.clone(),
                false,
            ),
            (p.run_dir.join("gitconfig.sock"), gitconfig_dir.clone(), true),
        ];
        for (socket, _dir, _readonly) in &specs {
            let _ = std::fs::remove_file(socket);
            virtiofsd_sockets.push(socket.clone());
        }
        for (socket, dir, readonly) in &specs {
            let log = p.run_dir.join(format!(
                "virtiofsd-{}.log",
                socket.file_name().unwrap().to_string_lossy()
            ));
            let _ = std::fs::write(&log, "");
            let child = start_virtiofsd(socket, dir, &log, *readonly, host_uid(), host_gid())
                .map_err(|e| RunnerError::Other(e.to_string()))?;
            children.virtiofsd.push(child);
        }

        let mut ready = false;
        for _ in 0..50 {
            if let Some(signal) = signal::take() {
                return Err(RunnerError::Interrupted(signal));
            }
            ready = virtiofsd_sockets.iter().all(|socket| socket_exists(socket));
            if ready {
                break;
            }
            for child in &mut children.virtiofsd {
                if child.try_wait().map_err(|e| RunnerError::Other(e.to_string()))?.is_some() {
                    dump_virtiofsd_logs(&p.run_dir);
                    return Err(RunnerError::Other(
                        "A virtiofs daemon exited before becoming ready.".to_string(),
                    ));
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if !ready {
            dump_virtiofsd_logs(&p.run_dir);
            return Err(RunnerError::Other(
                "Timed out waiting for the virtiofs daemons.".to_string(),
            ));
        }
    }

    if let Some(gateway) = &gateway_path {
        println!("Starting the gateway proxy...");
        let gateway_log = p.run_dir.join("gateway.log");
        let _ = std::fs::write(&gateway_log, "");
        let mut command = Command::new(format!("{gateway}/bin/aegis-gateway"));
        command
            .args(["serve", "--listen"])
            .arg(format!("127.0.0.1:{}", p.proxy_host_port))
            .args(["--ca-dir"])
            .arg(p.data_dir.join("ca"))
            .args(["--secrets"])
            .arg(&secrets_file)
            .args(["--age-key"])
            .arg(&age_key_file);
        let child = process::spawn_logged(command, &gateway_log)
            .map_err(|e| RunnerError::Other(e.to_string()))?;
        children.gateway = Some(child);
        process::wait_for_log(
            children.gateway.as_mut().unwrap(),
            &gateway_log,
            "listening on",
            SSH_WAIT_TIMEOUT,
        )
        .map_err(process_to_runner)?;
    }

    let vm_log = p.run_dir.join("vm.log");
    let _ = std::fs::write(&vm_log, "");

    println!("Booting the guest...");
    let vm_command_path = format!("{vm_path}/bin/run-aegis-vm");
    let mut image_lock: Option<FileLock> = None;
    if is_darwin() {
        image_lock = Some(FileLock::acquire_blocking(&p.image_lock_file)
            .map_err(|e| RunnerError::Other(e.to_string()))?);
        store_cache::link_shared_store_images(&p.state_dir, &p.cache_dir)
            .map_err(|e| RunnerError::Other(e.to_string()))?;
        let mut command = Command::new(&vm_command_path);
        command.env("VZVM_STATE_DIR", &p.state_dir);
        command.args(forwarded);
        let child = process::spawn_logged(command, &vm_log)
            .map_err(|e| RunnerError::Other(e.to_string()))?;
        children.vm = Some(child);
    } else {
        let mut command = Command::new(&vm_command_path);
        command.env(
            "QEMU_KERNEL_PARAMS",
            format!(
                "regInfo=/nix/.ro-store/{}/registration",
                reg_info.as_deref().unwrap_or_default()
            ),
        );
        command.env(
            "QEMU_OPTS",
            format!(
                "-smp {cpu} -m {mem} -object memory-backend-memfd,id=aegis-ram,size={mem}M,share=on -machine memory-backend=aegis-ram",
                cpu = vm.cpu,
                mem = vm.mem
            ),
        );
        command.args(forwarded);
        let child = process::spawn_logged(command, &vm_log)
            .map_err(|e| RunnerError::Other(e.to_string()))?;
        children.vm = Some(child);
    }

    if is_darwin() {
        let vm_child = children.vm.as_mut().expect("vm spawned");
        store_cache::wait_for_guest_start(vm_child, &vm_log)
            .map_err(guest_start_to_runner)?;
        store_cache::publish_store_images(&p.state_dir, &p.cache_dir)
            .map_err(|e| RunnerError::Other(e.to_string()))?;
        // The lock must cover publication so two runners sharing one cache
        // directory cannot race the rename and symlink of store images.
        drop(image_lock.take());
    }

    let ssh_target = if is_darwin() {
        vec![
            "-p".to_string(),
            p.ssh_port.to_string(),
            "agent@127.0.0.1".to_string(),
        ]
    } else {
        vec![format!("agent@vsock/{}", p.cid)]
    };
    let probe_target = if is_darwin() {
        vec![
            "-p".to_string(),
            p.ssh_port.to_string(),
            "root@127.0.0.1".to_string(),
        ]
    } else {
        vec![format!("root@vsock/{}", p.cid)]
    };

    println!("Waiting for the guest SSH server...");
    let probe_refs: Vec<&str> = probe_target.iter().map(String::as_str).collect();
    let vm_child = children.vm.as_mut().expect("vm spawned");
    process::wait_for_ssh(vm_child, &probe_refs, &p.ssh_key, &vm_log)
        .map_err(process_to_runner)?;

    if gateway_enabled {
        let tunnel_log = p.run_dir.join("reverse-tunnel.log");
        let _ = std::fs::write(&tunnel_log, "");
        let mut command = Command::new("ssh");
        command
            .arg("-v")
            .arg("-N")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes")
            .arg("-R")
            .arg(format!("3128:127.0.0.1:{}", p.proxy_host_port))
            .arg("-i")
            .arg(&p.ssh_key)
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-o")
            .arg("ConnectTimeout=2")
            .arg("-o")
            .arg("BatchMode=yes");
        command.args(&ssh_target);
        let child = process::spawn_logged(command, &tunnel_log)
            .map_err(|e| RunnerError::Other(e.to_string()))?;
        children.tunnel = Some(child);
        process::wait_for_log(
            children.tunnel.as_mut().unwrap(),
            &tunnel_log,
            "remote forward success",
            SSH_WAIT_TIMEOUT,
        )
        .map_err(process_to_runner)?;
    }

    let mut attach = Command::new("ssh");
    attach
        .arg("-tt")
        .arg("-i")
        .arg(&p.ssh_key)
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .args(&ssh_target)
        .arg("exec /run/current-system/sw/bin/opencode-shell")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut attach_child = attach.spawn().map_err(|e| RunnerError::Other(e.to_string()))?;

    loop {
        match attach_child.try_wait().map_err(|e| RunnerError::Other(e.to_string()))? {
            Some(status) => {
                children.kill_all();
                return Ok(ExitCode::from(status.code().unwrap_or(0).clamp(0, 255) as u8));
            }
            None => {
                if let Some(signal) = signal::take() {
                    let _ = attach_child.kill();
                    let _ = attach_child.wait();
                    children.kill_all();
                    return Ok(signal::exit_code(signal));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_checked_interrupts_on_signal() {
        let _guard = crate::signal::TEST_LOCK.lock().unwrap();
        crate::signal::set_for_test(libc::SIGINT);
        let mut command = Command::new("sleep");
        command.arg("10");
        assert!(matches!(
            run_checked(command),
            Err(RunnerError::Interrupted(libc::SIGINT))
        ));
    }
}
