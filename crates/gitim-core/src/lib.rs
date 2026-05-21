#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod auth_payload;
pub mod config_patch;
pub mod dm;
pub mod epoch;
pub mod flow;
pub mod formatter;
pub mod identity;
pub mod link;
pub mod me_json;
pub mod mention;
pub mod parser;
pub mod recipients;
pub mod responses;
// timer uses fs2 for advisory file locking, which has no wasm32 backend.
#[cfg(not(target_arch = "wasm32"))]
pub mod timer;
pub mod types;
pub mod validator;
