#![deny(warnings)]

pub mod agent;
pub mod agent_loop;
pub mod background;
pub mod context_window;
pub mod daemon_log;
pub mod email_propagation;
pub mod error;
pub mod git_config;
pub mod github;
pub mod gitignore;
pub mod http;
pub mod poller;
pub mod preflight;
pub mod slug;
pub mod state;
pub mod token_propagation;
pub mod update;
pub mod user_config;
pub mod workspace;

pub use agent::{provision_agent, AgentConfig, AgentHandle};
pub use agent_loop::{detect_steering_trigger, format_changes_as_prompt, AgentLoop};
pub use error::RuntimeError;
pub use poller::{ChannelChange, PollResult, Poller};
pub use state::AgentState;
