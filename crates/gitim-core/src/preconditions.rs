//! System/static invariant helpers.
//!
//! These functions document and enforce invariants guaranteed by literal regex
//! patterns (verified at compile time) or the Rust standard library / OS.
//! Panic on violation indicates a programming error (these should never fail
//! in correct usage).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use regex::Regex;

/// Compile a regex from a literal pattern verified at compile time.
#[track_caller]
pub fn regex_literal(pattern: &str) -> Regex {
    Regex::new(pattern).expect("regex literal verified at compile time")
}

/// Parse a `u64` from a string already validated by a regex capture group.
#[track_caller]
pub fn parse_u64(s: &str) -> u64 {
    s.parse().expect("regex already validated this is numeric")
}

/// Fill random bytes from the OS entropy source.
///
/// `getrandom` only fails in catastrophic situations (e.g. no OS entropy
/// source available). We treat this as unrecoverable.
pub fn random_bytes(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("OS entropy source unavailable")
}
