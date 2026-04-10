#![deny(warnings)]

pub mod agent;
pub mod agent_loop;
pub mod claude;
pub mod error;
pub mod poller;
pub mod state;

pub use agent::{provision_agent, AgentConfig, AgentHandle};
pub use agent_loop::AgentLoop;
pub use claude::{ClaudeResult, ClaudeSession};
pub use error::RuntimeError;
pub use poller::{ChannelChange, PollResult, Poller};
pub use state::AgentState;
