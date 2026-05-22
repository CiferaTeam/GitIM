//! System-library invariant helpers.
//!
//! These functions document and enforce invariants guaranteed by the Rust standard
//! library or tokio. Panic on violation indicates a programming error (these
//! should never fail in correct usage).
#![allow(clippy::expect_used)]

use std::sync::{Mutex, MutexGuard};
use tokio::process::Child as TokioChild;

/// Unwrap a piped stdout from a tokio Child. This is guaranteed to be `Some` because we always set
/// `Stdio::piped()` before calling `Command::stdout()`.
#[track_caller]
pub fn take_tokio_piped_stdout(child: &mut TokioChild) -> tokio::process::ChildStdout {
    child
        .stdout
        .take()
        .expect("stdout should be piped (Stdio::piped was set)")
}

/// Unwrap a piped stdin from a tokio Child. This is guaranteed to be `Some` because we always set
/// `Stdio::piped()` before calling `Command::stdin()`.
#[track_caller]
pub fn take_tokio_piped_stdin(child: &mut TokioChild) -> tokio::process::ChildStdin {
    child
        .stdin
        .take()
        .expect("stdin should be piped (Stdio::piped was set)")
}

/// Unwrap a piped stderr from a tokio Child. This is guaranteed to be `Some` because we always set
/// `Stdio::piped()` before calling `Command::stderr()`.
#[track_caller]
pub fn take_tokio_piped_stderr(child: &mut TokioChild) -> tokio::process::ChildStderr {
    child
        .stderr
        .take()
        .expect("stderr should be piped (Stdio::piped was set)")
}

/// Unwrap a mutex lock. This only fails if the mutex is poisoned (a prior
/// thread panicked while holding the lock), which we treat as unrecoverable.
#[track_caller]
pub fn mutex_lock<'a, T>(guard: &'a Mutex<T>) -> MutexGuard<'a, T> {
    guard.lock().expect("mutex should not be poisoned")
}

/// Unwrap a mutex inside an Arc. Same semantics as mutex_lock.
#[track_caller]
pub fn mutex_lock_arc<'a, T>(guard: &'a std::sync::Arc<Mutex<T>>) -> MutexGuard<'a, T> {
    guard.lock().expect("mutex should not be poisoned")
}

/// Serialize a static JSON value to bytes. This is infallible for any Value
/// constructed from `serde_json::json!()` or similar static literals — the
/// Serialize impl for Value never produces an error.
pub fn static_json_to_vec(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("static JSON serialization is infallible")
}
