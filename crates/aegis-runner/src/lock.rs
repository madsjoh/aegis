//! Whole file advisory locking via `flock`.

use std::fs::{File, TryLockError};
use std::path::Path;

/// A held exclusive advisory lock on a file. Released when dropped.
pub struct FileLock {
    file: File,
}

impl FileLock {
    /// Acquire the lock without blocking, returning `None` when another holder
    /// owns it. Creates the lock file when it does not exist.
    pub fn acquire(path: &Path) -> std::io::Result<Option<FileLock>> {
        let file = File::create(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(FileLock { file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Acquire the lock, blocking until it is available.
    pub fn acquire_blocking(path: &Path) -> std::io::Result<FileLock> {
        let file = File::create(path)?;
        file.lock()?;
        Ok(FileLock { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_lock(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-lock-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("lock")
    }

    #[test]
    fn acquire_creates_file_and_excludes_a_second_holder() {
        let path = temp_lock("exclude");
        let first = FileLock::acquire(&path).unwrap();
        assert!(first.is_some());
        assert!(path.exists());

        let second = FileLock::acquire(&path).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn acquire_blocking_waits_for_the_holder() {
        let path = temp_lock("blocking");
        let first = FileLock::acquire(&path).unwrap().unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let path_clone = path.clone();
        let handle = std::thread::spawn(move || {
            let _lock = FileLock::acquire_blocking(&path_clone).unwrap();
            tx.send(()).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            rx.try_recv().is_err(),
            "blocking acquire should wait for the holder"
        );

        drop(first);

        rx.recv_timeout(std::time::Duration::from_secs(2))
            .expect("blocking acquire should complete after the holder drops");
        handle.join().unwrap();
    }

    #[test]
    fn dropping_the_lock_permits_reacquisition() {
        let path = temp_lock("reacquire");
        let first = FileLock::acquire(&path).unwrap().unwrap();
        drop(first);

        let second = FileLock::acquire(&path).unwrap();
        assert!(second.is_some());
    }

    #[test]
    fn acquire_creates_the_file_when_missing() {
        let path = temp_lock("create");
        assert!(!path.exists());
        let _lock = FileLock::acquire(&path).unwrap().unwrap();
        assert!(path.exists());
    }
}
