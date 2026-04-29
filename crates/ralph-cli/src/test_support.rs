use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

thread_local! {
    // Depth counter so wave-test helpers that nest calls which both need the
    // cwd serial lock do not deadlock on the non-reentrant Mutex. See
    // `cwd_serial_guard` below.
    static CWD_SERIAL_DEPTH: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

/// RAII guard for the shared cwd serial lock used to serialize tests that
/// mutate process cwd (`CwdGuard`) with wave tests that snapshot cwd for a
/// PTY child. Reentrant via a thread-local depth counter: nested calls on
/// the same thread become no-ops rather than deadlocking on the underlying
/// non-reentrant Mutex.
pub(crate) enum CwdSerialGuard {
    Owned(#[allow(dead_code)] MutexGuard<'static, ()>),
    Reentrant,
}

impl Drop for CwdSerialGuard {
    fn drop(&mut self) {
        CWD_SERIAL_DEPTH.with(|d| {
            let current = d.get();
            debug_assert!(current > 0, "CwdSerialGuard depth underflow");
            d.set(current.saturating_sub(1));
        });
    }
}

/// Acquire the cwd serial lock. Safe to call even when a caller already
/// holds the lock on the current thread — the inner call becomes a no-op.
pub(crate) fn cwd_serial_guard() -> CwdSerialGuard {
    let was_held = CWD_SERIAL_DEPTH.with(|d| {
        let prev = d.get();
        d.set(prev + 1);
        prev > 0
    });
    if was_held {
        CwdSerialGuard::Reentrant
    } else {
        CwdSerialGuard::Owned(test_lock())
    }
}

pub(crate) fn safe_current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| {
        let fallback = std::env::temp_dir();
        std::env::set_current_dir(&fallback).expect("set fallback cwd");
        fallback
    })
}

pub(crate) struct CwdGuard {
    _lock: CwdSerialGuard,
    original: PathBuf,
}

impl CwdGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = cwd_serial_guard();
        let original = safe_current_dir();
        std::env::set_current_dir(path).expect("set current dir");
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}
