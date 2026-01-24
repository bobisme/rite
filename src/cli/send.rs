use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::channel::{dm_channel_name, is_valid_channel_name};
use crate::core::identity::{format_export, resolve_agent};
use crate::core::message::Message;
use crate::core::project::channel_path;
use crate::storage::jsonl::append_record;

/// Send a message to a channel or agent.
pub fn run(
    target: String,
    message: String,
    _meta: Option<String>,
    agent: Option<&str>,
    project_root: &Path,
) -> Result<()> {
    // Get current agent from env var or explicit flag
    let agent_name = resolve_agent(agent, project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "No agent identity configured.\n\n\
             Set your identity with: {}\n\
             Or use --agent flag.",
            format_export("YourAgentName")
        )
    })?;

    // Determine channel name
    let channel = if target.starts_with('@') {
        // DM to another agent
        let other_agent = target.trim_start_matches('@');
        if other_agent.is_empty() {
            bail!("Invalid DM target: {}", target);
        }
        dm_channel_name(&agent_name, other_agent)
    } else {
        // Regular channel
        if !is_valid_channel_name(&target) {
            bail!(
                "Invalid channel name: '{}'\n\n\
                 Channel names must be lowercase alphanumeric with hyphens.",
                target
            );
        }
        target.clone()
    };

    // Create and send the message
    let msg = Message::new(&agent_name, &channel, &message);

    let path = channel_path(project_root, &channel);
    append_record(&path, &msg)
        .with_context(|| format!("Failed to send message to #{}", channel))?;

    // Output confirmation
    if target.starts_with('@') {
        println!("{} Message sent to {}", "Sent:".green(), target.cyan());
    } else {
        println!("{} Message sent to #{}", "Sent:".green(), channel.cyan());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init;
    use crate::storage::jsonl::read_records;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_send_to_channel() {
        let temp = setup();

        // Use explicit agent name
        run(
            "general".to_string(),
            "Hello, world!".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        let messages: Vec<Message> = read_records(&channel_path(temp.path(), "general")).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].body, "Hello, world!");
        assert_eq!(messages[0].agent, "Sender");
    }

    #[test]
    fn test_send_to_new_channel() {
        let temp = setup();

        run(
            "backend".to_string(),
            "New channel!".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        let messages: Vec<Message> = read_records(&channel_path(temp.path(), "backend")).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].body, "New channel!");
    }

    #[test]
    fn test_send_dm() {
        let temp = setup();

        run(
            "@OtherAgent".to_string(),
            "Private message".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        // DM channel should be created with sorted names
        let dm_path = channel_path(temp.path(), "_dm_OtherAgent_Sender");
        let messages: Vec<Message> = read_records(&dm_path).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].body, "Private message");
    }

    #[test]
    fn test_send_invalid_channel() {
        let temp = setup();

        let result = run(
            "INVALID".to_string(),
            "test".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_send_without_registration() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        // No agent specified, no env var

        let result = run(
            "general".to_string(),
            "test".to_string(),
            None,
            None,
            temp.path(),
        );
        assert!(result.is_err());
    }
}
