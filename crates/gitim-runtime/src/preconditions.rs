//! System-library invariant helpers.
//!
//! These functions document and enforce invariants guaranteed by the Rust standard
//! library or well-known crates. Panic on violation indicates a programming error
//! (these should never fail in correct usage).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, MutexGuard};
use std::time::Duration;

use gitim_agent_provider::{create, ProviderConfig};

/// Unwrap a mutex lock. This only fails if the mutex is poisoned (a prior
/// thread panicked while holding the lock), which we treat as unrecoverable.
#[track_caller]
pub fn mutex_lock<'a, T>(guard: &'a std::sync::Mutex<T>) -> MutexGuard<'a, T> {
    guard.lock().expect("mutex should not be poisoned")
}

/// Unwrap an `Arc<Mutex<T>>` lock. Convenience wrapper for the common
/// `SharedRuntimeState` pattern.
#[track_caller]
pub fn arc_mutex_lock<'a, T>(guard: &'a Arc<std::sync::Mutex<T>>) -> MutexGuard<'a, T> {
    guard.lock().expect("mutex should not be poisoned")
}

/// Current Unix timestamp in seconds. System clock must be after the Unix epoch.
pub fn unix_timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after unix epoch")
        .as_secs()
}

/// Serialize a value to pretty-printed JSON. Infallible for types that implement
/// `Serialize` with standard derive (e.g. `serde_json::Value` or plain structs).
pub fn json_to_string_pretty<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .expect("JSON serialization is infallible for standard types")
}

/// Take piped stdin from a tokio child process. Guaranteed `Some` because
/// `Stdio::piped()` is always set before spawning.
pub fn take_tokio_piped_stdin(child: &mut tokio::process::Child) -> tokio::process::ChildStdin {
    child
        .stdin
        .take()
        .expect("stdin should be piped (Stdio::piped was set)")
}

/// Take piped stdout from a tokio child process. Guaranteed `Some` because
/// `Stdio::piped()` is always set before spawning.
pub fn take_tokio_piped_stdout(child: &mut tokio::process::Child) -> tokio::process::ChildStdout {
    child
        .stdout
        .take()
        .expect("stdout should be piped (Stdio::piped was set)")
}

/// Build a `NaiveDateTime` from literal components. Used for calendar-month
/// boundaries where year/month/day are derived from `Utc::now()` and are
/// guaranteed to form a valid date.
pub fn naive_datetime(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
) -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(year, month, day)
        .expect("valid date literal")
        .and_hms_opt(hour, min, sec)
        .expect("valid time literal")
}

/// Build a `reqwest::Client` with standard timeout settings. Infallible because
/// the builder only uses well-known settings that are guaranteed valid.
pub fn reqwest_client_with_defaults(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .expect("reqwest client builds with default settings")
}

/// Compile a regex pattern. Infallible for statically-known patterns.
#[track_caller]
pub fn regex_compile(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("static regex pattern compiles")
}

/// Create the built-in hermes provider. Infallible because hermes is a
/// built-in provider that is always registered.
pub fn hermes_provider() -> Box<dyn gitim_agent_provider::Provider> {
    create("hermes", ProviderConfig::default()).expect("hermes is a built-in provider")
}
