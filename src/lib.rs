pub mod cli;
pub mod core;
pub mod index;
pub mod storage;
pub mod tui;

// Re-export commonly used types
pub use core::{Agent, AgentEvent, ClaimEvent, FileClaim, Message, MessageMeta, SystemEvent};
pub use index::{IndexSyncer, SearchIndex};
pub use storage::{ProjectState, State};
