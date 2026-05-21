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

use chrono::{SubsecRound, Utc};
use gitim_core::timer::{
    cancel_timer, find_clone_root, parse_duration, read_timers, register_timer, TimerError,
};
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
    // Whole-seconds for human display. Sub-second precision is noise at
    // 10s-24h timer scale and overflows the list column elsewhere.
    let remaining = (timer.fire_at - Utc::now()).to_std().unwrap_or_default();
    let remaining_secs = std::time::Duration::from_secs(remaining.as_secs());
    println!(
        "{}  fires in {}  (at {})",
        timer.id,
        format_duration(remaining_secs),
        timer.fire_at.trunc_subsecs(0).to_rfc3339()
    );
}

pub async fn cmd_list(_mode: &OutputMode, json: bool) {
    let clone = match resolve_clone() {
        Ok(c) => c,
        Err(e) => exit_with(&e, 2),
    };
    let mut file = match read_timers(&clone) {
        Ok(f) => f,
        Err(e) => exit_with(&e, 1),
    };
    file.timers.sort_by_key(|t| t.fire_at);

    if json {
        match serde_json::to_string_pretty(&file.timers) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        return;
    }

    if file.timers.is_empty() {
        println!("(no pending timers)");
        return;
    }

    let now = Utc::now();
    println!(
        "{:<28} {:<10} {:<25} {:<28} NOTE",
        "ID", "DUE IN", "FIRES AT", "ANCHOR"
    );
    for t in &file.timers {
        let due_in = (t.fire_at - now).to_std().unwrap_or_default();
        let due_secs = std::time::Duration::from_secs(due_in.as_secs());
        let due_fmt = trim_humantime(format_duration(due_secs).to_string());
        println!(
            "{:<28} {:<10} {:<25} {:<28} {}",
            t.id,
            due_fmt,
            t.fire_at.trunc_subsecs(0).to_rfc3339(),
            t.anchor,
            t.note.as_deref().unwrap_or("")
        );
    }
}

pub async fn cmd_cancel(_mode: &OutputMode, id_or_prefix: &str) {
    let clone = match resolve_clone() {
        Ok(c) => c,
        Err(e) => exit_with(&e, 2),
    };
    match cancel_timer(&clone, id_or_prefix) {
        Ok(id) => println!("cancelled: {id}"),
        Err(e @ (TimerError::NoMatch(_) | TimerError::AmbiguousPrefix { .. })) => exit_with(&e, 2),
        Err(e) => exit_with(&e, 1),
    }
}

fn resolve_clone() -> Result<std::path::PathBuf, TimerError> {
    let cwd = std::env::current_dir().map_err(TimerError::Io)?;
    find_clone_root(&cwd)
}

fn exit_with(e: &TimerError, code: i32) -> ! {
    eprintln!("error: {e}");
    process::exit(code);
}

/// Trim humantime output ("30m 0s 0ms") to the leading unit pair
/// ("30m 0s") so the list column stays compact.
fn trim_humantime(s: String) -> String {
    s.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}
