//! Mark channel as read command.

use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::identity::resolve_agent;
use crate::core::project::channel_path;
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
pub fn run(
    options: MarkReadOptions,
    explicit_agent: Option<&str>,
    project_root: &Path,
) -> Result<()> {
    let agent = resolve_agent(explicit_agent, project_root)
        .context("Could not determine agent identity")?;

    let channel_file = channel_path(project_root, &options.channel);

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
    let manager = AgentStateManager::new(project_root, &agent);
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
    use super::*;
    use crate::cli::{init, send};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_mark_read_basic() {
        let temp = setup();

        // Send a message first
        send::run(
            "general".to_string(),
            "Test message".to_string(),
            None,
            Some("TestAgent"),
            temp.path(),
        )
        .unwrap();

        // Mark as read
        let options = MarkReadOptions {
            channel: "general".to_string(),
            offset: None,
            last_id: None,
        };
        run(options, Some("TestAgent"), temp.path()).unwrap();

        // Verify state was saved
        let manager = AgentStateManager::new(temp.path(), "TestAgent");
        let cursor = manager.get_read_cursor("general").unwrap();
        assert!(cursor.offset > 0);
        assert!(cursor.last_id.is_some());
    }

    #[test]
    fn test_mark_read_explicit_offset() {
        let temp = setup();

        // Send a message first
        send::run(
            "general".to_string(),
            "Test message".to_string(),
            None,
            Some("TestAgent"),
            temp.path(),
        )
        .unwrap();

        // Mark as read with explicit offset
        let options = MarkReadOptions {
            channel: "general".to_string(),
            offset: Some(50),
            last_id: Some("01CUSTOM".to_string()),
        };
        run(options, Some("TestAgent"), temp.path()).unwrap();

        // Verify state was saved with our values
        let manager = AgentStateManager::new(temp.path(), "TestAgent");
        let cursor = manager.get_read_cursor("general").unwrap();
        assert_eq!(cursor.offset, 50);
        assert_eq!(cursor.last_id, Some("01CUSTOM".to_string()));
    }

    #[test]
    fn test_mark_read_nonexistent_channel() {
        let temp = setup();

        let options = MarkReadOptions {
            channel: "nonexistent".to_string(),
            offset: None,
            last_id: None,
        };
        let result = run(options, Some("TestAgent"), temp.path());
        assert!(result.is_err());
    }
}
