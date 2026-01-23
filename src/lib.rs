pub mod cli;
pub mod core;
pub mod storage;

// Re-export commonly used types
pub use core::{Agent, AgentEvent, ClaimEvent, FileClaim, Message, MessageMeta, SystemEvent};
pub use storage::{ProjectState, State};
