#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod conflict;
#[cfg(not(target_arch = "wasm32"))]
pub mod git;
pub mod renumber;
#[cfg(not(target_arch = "wasm32"))]
pub mod rotate;
#[cfg(not(target_arch = "wasm32"))]
pub mod sync_loop;
pub mod url_redact;
#[cfg(not(target_arch = "wasm32"))]
pub mod watcher;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod test_util;
