//! Subprocess helpers for the runner.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// An error while managing a child process.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("the child process exited before becoming ready")]
    Exited,
    #[error("timed out waiting for readiness")]
    Timeout,
    #[error("the VM exited before SSH became available")]
    VmExited,
    #[error("failed to spawn the child: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("failed to probe the child: {0}")]
    Probe(#[source] std::io::Error),
    #[error("failed to open the log file: {0}")]
    ReadLog(#[source] std::io::Error),
}

/// Spawn `command` with stdout and stderr appended to `log`.
pub fn spawn_logged(mut command: Command, log: &Path) -> Result<Child, ProcessError> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(ProcessError::ReadLog)?;
    let stderr = file.try_clone().map_err(ProcessError::ReadLog)?;
    let child = command
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(ProcessError::Spawn)?;
    Ok(child)
}

/// Poll `log` until `needle` appears, failing when the child exits or on
/// timeout.
pub fn wait_for_log(
    child: &mut Child,
    log: &Path,
    needle: &str,
    timeout: Duration,
) -> Result<(), ProcessError> {
    let start = Instant::now();
    loop {
        if child.try_wait().map_err(ProcessError::Probe)?.is_some() {
            return Err(ProcessError::Exited);
        }
        if log_contains(log, needle)? {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(ProcessError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Probe the guest SSH server until it answers, failing when the VM exits.
pub fn wait_for_ssh(
    child: &mut Child,
    target: &[&str],
    key: &Path,
    log: &Path,
) -> Result<(), ProcessError> {
    let mut failed_probes = 0u64;
    loop {
        let mut command = Command::new("ssh");
        command
            .arg("-i")
            .arg(key)
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-o")
            .arg("ConnectTimeout=2")
            .arg("-o")
            .arg("BatchMode=yes");
        command.args(target);
        command.arg("true");
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());

        if command.status().map_err(ProcessError::Probe)?.success() {
            return Ok(());
        }

        if child.try_wait().map_err(ProcessError::Probe)?.is_some() {
            eprintln!("Error: The Aegis VM exited before SSH became available.");
            if let Ok(content) = std::fs::read_to_string(log) {
                eprint!("{content}");
            }
            return Err(ProcessError::VmExited);
        }

        failed_probes += 1;
        if failed_probes % 150 == 0 {
            println!("Still waiting for the guest SSH server...");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn log_contains(log: &Path, needle: &str) -> Result<bool, ProcessError> {
    match std::fs::read_to_string(log) {
        Ok(content) => Ok(content.contains(needle)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ProcessError::ReadLog(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-process-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(
            &path,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        path
    }

    #[test]
    fn wait_for_log_returns_ok_when_the_needle_appears() {
        let dir = temp_dir("log-ok");
        let log = dir.join("gateway.log");
        let writer = write_executable(
            &dir,
            "writer",
            "#!/bin/sh\nsleep 0.1\nprintf 'listening on 127.0.0.1\\n'\nsleep 1\n",
        );
        let mut child = spawn_logged(Command::new(writer), &log).unwrap();

        wait_for_log(&mut child, &log, "listening on", Duration::from_secs(5)).unwrap();

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn wait_for_log_fails_when_the_child_exits() {
        let dir = temp_dir("log-exited");
        let log = dir.join("gateway.log");
        let mut child = Command::new("true").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        assert!(matches!(
            wait_for_log(&mut child, &log, "listening on", Duration::from_secs(5)),
            Err(ProcessError::Exited)
        ));
    }

    #[test]
    fn wait_for_ssh_returns_ok_when_the_probe_succeeds() {
        let _guard = PATH_LOCK.lock().unwrap();

        let dir = temp_dir("ssh-ok");
        let key = dir.join("key");
        std::fs::write(&key, "unused").unwrap();
        let log = dir.join("vm.log");
        let ssh = write_executable(&dir, "ssh", "#!/bin/sh\nexit 0\n");

        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{original_path}", dir.display()));

        let mut vm = Command::new("sleep").arg("1").spawn().unwrap();
        let result = wait_for_ssh(&mut vm, &["root@vsock/3"], &key, &log);

        std::env::set_var("PATH", original_path);
        vm.kill().unwrap();
        vm.wait().unwrap();

        assert!(result.is_ok(), "got: {result:?}");
        let _ = ssh;
    }

    #[test]
    fn wait_for_ssh_fails_when_the_vm_exits() {
        let _guard = PATH_LOCK.lock().unwrap();

        let dir = temp_dir("ssh-vm-exited");
        let key = dir.join("key");
        std::fs::write(&key, "unused").unwrap();
        let log = dir.join("vm.log");
        std::fs::write(&log, "VM terminated during startup.\n").unwrap();
        let _ssh = write_executable(&dir, "ssh", "#!/bin/sh\nexit 1\n");

        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{original_path}", dir.display()));

        let mut vm = Command::new("sleep").arg("0.1").spawn().unwrap();

        let result = wait_for_ssh(&mut vm, &["root@vsock/3"], &key, &log);

        std::env::set_var("PATH", original_path);
        vm.wait().unwrap();

        assert!(matches!(result, Err(ProcessError::VmExited)), "got: {result:?}");
    }
}
