//! Cooperative signal handling for the runner.

use std::process::ExitCode;
use std::sync::atomic::{AtomicI32, Ordering};

static SIGNAL_FLAG: AtomicI32 = AtomicI32::new(0);

/// Install handlers that record SIGINT, SIGTERM, and SIGHUP on the flag.
pub fn install() {
    let handler: extern "C" fn(libc::c_int) = handle_signal;
    unsafe {
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGHUP, handler as libc::sighandler_t);
    }
}

/// Take the pending signal, clearing the flag, or `None` when none is pending.
pub fn take() -> Option<i32> {
    match SIGNAL_FLAG.swap(0, Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

/// The exit code the Bash runner used for each trapped signal.
pub fn exit_code(signal: i32) -> ExitCode {
    match signal {
        libc::SIGHUP => ExitCode::from(129),
        libc::SIGINT => ExitCode::from(130),
        libc::SIGTERM => ExitCode::from(143),
        _ => ExitCode::FAILURE,
    }
}

extern "C" fn handle_signal(signal: libc::c_int) {
    SIGNAL_FLAG.store(signal, Ordering::SeqCst);
}

#[cfg(test)]
pub fn set_for_test(signal: i32) {
    SIGNAL_FLAG.store(signal, Ordering::SeqCst);
}

#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_maps_signals() {
        assert_eq!(exit_code(libc::SIGHUP), std::process::ExitCode::from(129));
        assert_eq!(exit_code(libc::SIGINT), std::process::ExitCode::from(130));
        assert_eq!(exit_code(libc::SIGTERM), std::process::ExitCode::from(143));
    }

    #[test]
    fn take_returns_none_when_clear() {
        let _guard = TEST_LOCK.lock().unwrap();
        assert_eq!(take(), None);
    }

    #[test]
    fn take_returns_and_clears_the_pending_signal() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_for_test(libc::SIGINT);
        assert_eq!(take(), Some(libc::SIGINT));
        assert_eq!(take(), None);
    }
}
