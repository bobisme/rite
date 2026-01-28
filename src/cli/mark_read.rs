//! Mark channel as read command.

use anyhow::{Result, bail};
use colored::Colorize;

use crate::core::identity::require_agent;
use crate::core::project::{channel_path, data_dir};
use crate::storage::agent_state::AgentStateManager;

pub struct MarkReadOptions {
    /// Channel to mark as read
    pub channel: String,
    /// Explicit byte offset (if not provided, uses current file size)
    pub offset: Option<u64>,
    /// Explicit message ID to mark as last read
    pub last_id: Option<String>,
}

/// Mark a channel as read for the current agent.
pub fn run(options: MarkReadOptions, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;

    let channel_file = channel_path(&options.channel);

    // Get the offset to use
    let offset = if let Some(o) = options.offset {
        o
    } else if channel_file.exists() {
        std::fs::metadata(&channel_file)
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        bail!(
            "Channel #{} does not exist. Nothing to mark as read.",
            options.channel
        );
    };

    // Get the last message ID if not explicitly provided
    let last_id = if options.last_id.is_some() {
        options.last_id.clone()
    } else if channel_file.exists() {
        // Read last message to get its ID
        use crate::core::message::Message;
        use crate::storage::jsonl::read_last_n;

        let messages: Vec<Message> = read_last_n(&channel_file, 1).unwrap_or_default();
        messages.last().map(|m| m.id.to_string())
    } else {
        None
    };

    // Save read state
    let manager = AgentStateManager::new(&data_dir(), &agent);
    manager.mark_read(&options.channel, offset, last_id.as_deref())?;

    println!(
        "{} marked #{} as read at offset {}{}",
        "✓".green(),
        options.channel.cyan(),
        offset,
        if let Some(id) = &last_id {
            format!(" (last_id: {})", id)
        } else {
            String::new()
        }
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    // Integration tests moved to tests/integration/ since they require
    // global data directory mocking
}
