#![deny(warnings)]

pub mod conflict;
#[cfg(not(target_arch = "wasm32"))]
pub mod git;
pub mod renumber;
#[cfg(not(target_arch = "wasm32"))]
pub mod sync_loop;
pub mod url_redact;
#[cfg(not(target_arch = "wasm32"))]
pub mod watcher;
