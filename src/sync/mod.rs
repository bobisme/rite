//! Git-based multi-machine sync for BotBus data directory.
//!
//! Enables transparent auto-commit and manual sync commands to sync
//! BotBus data across machines using git with union merge strategy.

pub mod auto_commit;
pub mod git;

pub use auto_commit::auto_commit_after_claim;
pub use auto_commit::auto_commit_after_release;
pub use auto_commit::auto_commit_after_send;
