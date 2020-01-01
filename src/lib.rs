pub mod attachments;
pub mod cli;
pub mod core;
pub mod index;
pub mod storage;
pub mod sync;
pub mod telegram;
pub mod telemetry;
pub mod tui;

// Re-export commonly used types
pub use core::{
    Agent, AgentEvent, ClaimEvent, FileClaim, Hook, HookCondition, HookFiring, Message,
    MessageMeta, SystemEvent,
};
pub use index::{IndexSyncer, SearchIndex};
pub use storage::{ProjectState, State};
