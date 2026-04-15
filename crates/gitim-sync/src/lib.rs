#![deny(warnings)]

#[cfg(not(target_arch = "wasm32"))]
pub mod git;
#[cfg(not(target_arch = "wasm32"))]
pub mod watcher;
#[cfg(not(target_arch = "wasm32"))]
pub mod sync_loop;
pub mod renumber;
pub mod conflict;
