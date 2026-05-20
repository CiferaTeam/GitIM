//! `gitim timer` subcommands.
//!
//! Pure-fs operations against `<clone>/.gitim/timers.json`. No daemon
//! involvement: `main.rs` short-circuits `Commands::Timer` before
//! `init_client()` so these commands never spawn / contact the daemon.
//!
//! Exit codes:
//! - `0` success
//! - `1` IO / serde / lock failure (unexpected)
//! - `2` user / validation error (invalid duration, empty anchor, cap
//!   reached, no match, ambiguous prefix, not in a gitim clone)

use std::process;

use chrono::Utc;
use gitim_core::timer::{find_clone_root, parse_duration, register_timer, TimerError};
use humantime::format_duration;

use crate::output::OutputMode;

pub async fn cmd_set(_mode: &OutputMode, duration: &str, anchor: &str, note: Option<&str>) {
    let dur = match parse_duration(duration) {
        Ok(d) => d,
        Err(e) => exit_with(&e, 2),
    };
    let clone = match resolve_clone() {
        Ok(c) => c,
        Err(e) => exit_with(&e, 2),
    };
    let timer = match register_timer(&clone, dur, anchor.to_string(), note.map(|s| s.to_string())) {
        Ok(t) => t,
        Err(e) => match e {
            TimerError::CapReached
            | TimerError::EmptyAnchor
            | TimerError::InvalidDuration(_)
            | TimerError::NotInClone => exit_with(&e, 2),
            _ => exit_with(&e, 1),
        },
    };
    let remaining = (timer.fire_at - Utc::now()).to_std().unwrap_or_default();
    println!(
        "{}  fires in {}  (at {})",
        timer.id,
        format_duration(remaining),
        timer.fire_at.to_rfc3339()
    );
}

fn resolve_clone() -> Result<std::path::PathBuf, TimerError> {
    let cwd = std::env::current_dir().map_err(TimerError::Io)?;
    find_clone_root(&cwd)
}

fn exit_with(e: &TimerError, code: i32) -> ! {
    eprintln!("error: {e}");
    process::exit(code);
}
