//! System-library invariant helpers.
//!
//! These functions document and enforce invariants guaranteed by the Rust standard
//! library. Panic on violation indicates a programming error (these should never fail
//! in correct usage).
#![allow(clippy::expect_used)]

use std::sync::MutexGuard;

/// Unwrap a mutex lock. This only fails if the mutex is poisoned (a prior
/// thread panicked while holding the lock), which we treat as unrecoverable.
#[track_caller]
pub fn mutex_lock<'a, T>(guard: &'a std::sync::Mutex<T>) -> MutexGuard<'a, T> {
    guard.lock().expect("mutex should not be poisoned")
}
