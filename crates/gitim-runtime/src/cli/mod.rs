//! Shared infrastructure for the `gitim-runtime` one-shot CLI.
//!
//! Subcommand handlers (status, list-agents, add-agent, etc. — landing in
//! tasks 6-12 of the runtime-cli plan) all share:
//! - HTTP client with port discovery + structured error classification (`http`)
//! - Workspace selection / disambiguation (`workspace`)
//! - Exit-code mapping from `CliError` variants (`exit_code`)
//!
//! Pulling the most-used types up here so subcommand modules can write
//! `use crate::cli::{Client, CliError};` without three separate imports.

pub mod cmd_list_agents;
pub mod cmd_runtime_id;
pub mod cmd_status;
pub mod cmd_workspaces;
pub mod dto;
pub mod exit_code;
pub mod http;
pub mod workspace;

pub use dto::{
    agent_detail_from_value, redact_env_secrets, AddAgentResponse, AgentDetail, AgentView,
    ErrorResponse, RuntimeStatus,
};
pub use exit_code::from_cli_error;
pub use http::{resolve_base_url, Client, CliError};
pub use workspace::{resolve_workspace, select_workspace};
