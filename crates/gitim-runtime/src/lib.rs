#![deny(warnings)]

pub mod agent;
pub mod agent_loop;
pub mod error;
pub mod git_config;
pub mod github;
pub mod http;
pub mod poller;
pub mod preflight;
pub mod slug;
pub mod state;
pub mod token_propagation;

pub use agent::{provision_agent, AgentConfig, AgentHandle};
pub use agent_loop::{AgentLoop, detect_steering_trigger, format_changes_as_prompt};
pub use error::RuntimeError;
pub use poller::{ChannelChange, PollResult, Poller};
pub use state::AgentState;
