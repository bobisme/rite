use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::identity::format_export;
use crate::core::message::{Message, MessageMeta, SystemEvent};
use crate::core::names::{generate_unique_name, is_valid_agent_name};
use crate::core::project::{agents_path, channel_path};
use crate::storage::jsonl::{append_record, read_records};

/// Register an agent identity in the current project.
pub fn run(name: Option<String>, description: Option<String>, project_root: &Path) -> Result<()> {
    // Load existing agents to check for duplicates
    let agents: Vec<Agent> =
        read_records(&agents_path(project_root)).with_context(|| "Failed to read agents")?;

    let agent_exists = |n: &str| agents.iter().any(|a| a.name == n);

    // Determine the agent name
    let agent_name = match name {
        Some(n) => {
            // Validate provided name
            if !is_valid_agent_name(&n) {
                bail!(
                    "Invalid agent name: '{}'\n\n\
                     Agent names must:\n\
                     - Start with a letter\n\
                     - Contain only alphanumeric characters and underscores\n\
                     - Be 1-64 characters long",
                    n
                );
            }

            // Check for duplicates
            if agent_exists(&n) {
                bail!("Agent name '{}' is already taken", n);
            }

            n
        }
        None => {
            // Generate a unique name
            generate_unique_name(agent_exists)
        }
    };

    // Create the agent record
    let mut agent = Agent::new(&agent_name);
    if let Some(desc) = description {
        agent = agent.with_description(desc);
    }

    // Append to agents.jsonl
    append_record(&agents_path(project_root), &agent)
        .with_context(|| "Failed to register agent")?;

    // Post join message to #general
    let join_msg = Message::new(
        &agent_name,
        "general",
        format!("{} has joined the project", agent_name),
    )
    .with_meta(MessageMeta::System {
        event: SystemEvent::AgentRegistered,
    });

    let general_path = channel_path(project_root, "general");
    append_record(&general_path, &join_msg).with_context(|| "Failed to post join message")?;

    // Output
    println!("{} Registered as {}", "Success:".green(), agent_name.cyan());

    if let Some(desc) = &agent.description {
        println!("  Description: {}", desc);
    }

    // Output export command for shell integration
    println!("\n{}", "Set your identity:".yellow());
    println!("  {}", format_export(&agent_name).cyan());

    println!("\n{}", "Then you can:".yellow());
    println!(
        "  {} Send a message",
        "botbus send general \"Hello!\"".cyan()
    );
    println!("  {} View your identity", "botbus whoami".cyan());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init;
    use tempfile::TempDir;

    fn setup_project() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_register_with_name() {
        let temp = setup_project();

        run(Some("TestAgent".to_string()), None, temp.path()).unwrap();

        // Check agent was registered
        let agents: Vec<Agent> = read_records(&agents_path(temp.path())).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "TestAgent");

        // Check join message was posted
        let messages: Vec<Message> = read_records(&channel_path(temp.path(), "general")).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].body.contains("TestAgent"));
    }

    #[test]
    fn test_register_auto_name() {
        let temp = setup_project();

        run(None, None, temp.path()).unwrap();

        let agents: Vec<Agent> = read_records(&agents_path(temp.path())).unwrap();
        assert_eq!(agents.len(), 1);
        assert!(!agents[0].name.is_empty());
    }

    #[test]
    fn test_register_with_description() {
        let temp = setup_project();

        run(
            Some("MyAgent".to_string()),
            Some("Claude Sonnet".to_string()),
            temp.path(),
        )
        .unwrap();

        let agents: Vec<Agent> = read_records(&agents_path(temp.path())).unwrap();
        assert_eq!(agents[0].description, Some("Claude Sonnet".to_string()));
    }

    #[test]
    fn test_register_duplicate_fails() {
        let temp = setup_project();

        run(Some("DupeAgent".to_string()), None, temp.path()).unwrap();
        let result = run(Some("DupeAgent".to_string()), None, temp.path());

        assert!(result.is_err());
    }

    #[test]
    fn test_register_invalid_name_fails() {
        let temp = setup_project();

        let result = run(Some("123Invalid".to_string()), None, temp.path());
        assert!(result.is_err());

        let result = run(Some("has-dash".to_string()), None, temp.path());
        assert!(result.is_err());
    }
}
