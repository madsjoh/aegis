//! Shared store image cache and guest start wait.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Child;

const GUEST_STARTED_MARKER: &str = "[vzvm] guest started";

/// An error while waiting for the guest to start.
#[derive(Debug, thiserror::Error)]
pub enum GuestStartError {
    #[error("the VM exited before the guest started")]
    VmExited,
    #[error("failed to probe the VM: {0}")]
    Probe(#[source] std::io::Error),
    #[error("interrupted by signal {0}")]
    Interrupted(i32),
}

/// Symlink every regular `store-*.img` file in `cache` into `state`.
pub fn link_shared_store_images(state: &Path, cache: &Path) -> std::io::Result<()> {
    let entries: Vec<_> = std::fs::read_dir(cache)?.collect::<Result<_, _>>()?;
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if !is_store_image(&name) || !path.is_file() {
            continue;
        }
        let dest = state.join(&name);
        let _ = std::fs::remove_file(&dest);
        std::os::unix::fs::symlink(&path, &dest)?;
    }
    Ok(())
}

/// Move non symlink `store-*.img` files from `state` into `cache` and replace
/// each with a symlink to the cached copy.
pub fn publish_store_images(state: &Path, cache: &Path) -> std::io::Result<()> {
    let entries: Vec<_> = std::fs::read_dir(state)?.collect::<Result<_, _>>()?;
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if !is_store_image(&name) || !path.is_file() || path.is_symlink() {
            continue;
        }
        let cache_path = cache.join(&name);
        std::fs::rename(&path, &cache_path)?;
        std::os::unix::fs::symlink(&cache_path, &path)?;
    }
    Ok(())
}

/// Wait until the exact line `[vzvm] guest started` appears in the log, failing
/// once the process with `pid` is gone.
pub fn wait_for_guest_start(child: &mut Child, log: &Path) -> Result<(), GuestStartError> {
    loop {
        if let Some(signal) = crate::signal::take() {
            return Err(GuestStartError::Interrupted(signal));
        }
        if child.try_wait().map_err(GuestStartError::Probe)?.is_some() {
            return Err(GuestStartError::VmExited);
        }
        if log_has_marker(log) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn is_store_image(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("store-") && name.ends_with(".img")
}

fn log_has_marker(log: &Path) -> bool {
    std::fs::read_to_string(log)
        .map(|content| content.lines().any(|line| line == GUEST_STARTED_MARKER))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-cache-{name}-{}-{}",
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
    fn link_shared_store_images_links_cache_images() {
        let state = temp_dir("link-state");
        let cache = temp_dir("link-cache");
        std::fs::write(cache.join("store-current.img"), "data").unwrap();

        link_shared_store_images(&state, &cache).unwrap();

        let link = state.join("store-current.img");
        assert!(link.is_symlink());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            cache.join("store-current.img")
        );
        assert!(!state.join("vzvm.json").exists());
    }

    #[test]
    fn publish_store_images_moves_and_links_a_new_image() {
        let state = temp_dir("publish-state");
        let cache = temp_dir("publish-cache");
        std::fs::write(state.join("store-new.img"), "data").unwrap();

        publish_store_images(&state, &cache).unwrap();

        assert!(cache.join("store-new.img").is_file());
        let link = state.join("store-new.img");
        assert!(link.is_symlink());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            cache.join("store-new.img")
        );
    }

    #[test]
    fn publish_store_images_leaves_symlinks_alone() {
        let state = temp_dir("publish-symlink-state");
        let cache = temp_dir("publish-symlink-cache");
        std::fs::write(cache.join("store-existing.img"), "data").unwrap();
        std::os::unix::fs::symlink(
            cache.join("store-existing.img"),
            state.join("store-existing.img"),
        )
        .unwrap();

        publish_store_images(&state, &cache).unwrap();

        let link = state.join("store-existing.img");
        assert!(link.is_symlink());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            cache.join("store-existing.img")
        );
    }

    #[test]
    fn wait_for_guest_start_ignores_partial_matches() {
        let log = temp_dir("partial").join("vm.log");
        let mut child = Command::new("sleep").arg("10").spawn().unwrap();

        std::fs::write(&log, "prefix [vzvm] guest started suffix\n").unwrap();

        let log2 = log.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let result = wait_for_guest_start(&mut child, &log2);
            let _ = child.kill();
            let _ = child.wait();
            tx.send(result.is_ok()).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "partial line must not satisfy the wait");

        std::fs::write(&log, "[vzvm] guest started\n").unwrap();
        assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap());
        waiter.join().unwrap();
    }

    #[test]
    fn wait_for_guest_start_fails_when_the_process_is_a_zombie() {
        let log = temp_dir("zombie").join("vm.log");
        let mut child = Command::new("true").spawn().unwrap();
        assert!(matches!(
            wait_for_guest_start(&mut child, &log),
            Err(GuestStartError::VmExited)
        ));
    }

    #[test]
    fn wait_for_guest_start_reports_vm_exited_even_after_the_marker() {
        let log = temp_dir("marker-dead").join("vm.log");
        let mut child = Command::new("sleep").arg("0.1").spawn().unwrap();
        std::fs::write(&log, "[vzvm] guest started\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(matches!(
            wait_for_guest_start(&mut child, &log),
            Err(GuestStartError::VmExited)
        ));
    }

    #[test]
    fn wait_for_guest_start_returns_ok_on_the_exact_marker() {
        let log = temp_dir("success").join("vm.log");
        let mut child = Command::new("sleep").arg("10").spawn().unwrap();

        let log2 = log.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::write(&log2, "[vzvm] guest started\n").unwrap();
        });

        wait_for_guest_start(&mut child, &log).unwrap();
        handle.join().unwrap();

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn wait_for_guest_start_returns_interrupted_when_a_signal_is_pending() {
        let _guard = crate::signal::TEST_LOCK.lock().unwrap();

        let log = temp_dir("interrupted").join("vm.log");
        let mut child = Command::new("sleep").arg("10").spawn().unwrap();
        crate::signal::set_for_test(libc::SIGINT);

        assert!(matches!(
            wait_for_guest_start(&mut child, &log),
            Err(GuestStartError::Interrupted(libc::SIGINT))
        ));

        let _ = child.kill();
        let _ = child.wait();
    }
}
