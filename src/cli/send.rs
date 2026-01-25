//! Send messages to channels or agents.

use anyhow::{bail, Context, Result};
use colored::Colorize;

use crate::core::channel::{dm_channel_name, is_valid_channel_name};
use crate::core::identity::require_agent;
use crate::core::message::{Attachment, Message};
use crate::core::project::channel_path;
use crate::storage::jsonl::append_record;

/// Simple message send (no labels or attachments) - for internal use and tests.
pub fn run_simple(target: String, message: String, agent: Option<&str>) -> Result<()> {
    run(target, message, None, vec![], vec![], agent)
}

/// Send a message to a channel or agent.
pub fn run(
    target: String,
    message: String,
    _meta: Option<String>,
    labels: Vec<String>,
    attachments: Vec<String>,
    agent: Option<&str>,
) -> Result<()> {
    // Get current agent from env var or explicit flag
    let agent_name = require_agent(agent)?;

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
                 Channel names must be lowercase alphanumeric with hyphens.\n\
                 Examples: general, backend, webapp-api, project-topic",
                target
            );
        }
        target.clone()
    };

    // Parse attachments (format: "name:path" or just "path")
    let parsed_attachments = parse_attachments(&attachments)?;

    // Create and send the message
    let mut msg = Message::new(&agent_name, &channel, &message);

    if !labels.is_empty() {
        msg = msg.with_labels(labels);
    }

    if !parsed_attachments.is_empty() {
        msg = msg.with_attachments(parsed_attachments);
    }

    let path = channel_path(&channel);
    append_record(&path, &msg)
        .with_context(|| format!("Failed to send message to #{}", channel))?;

    // Output confirmation
    if target.starts_with('@') {
        println!("{} Message sent to {}", "Sent:".green(), target.cyan());
        // Tip for DMs - mention the wait command
        println!(
            "{}",
            format!("Tip: botbus wait -c {} -t 60 to wait for reply", target).dimmed()
        );
    } else {
        println!("{} Message sent to #{}", "Sent:".green(), channel.cyan());
    }

    Ok(())
}

/// Parse attachment specifications.
/// Format: "name:path", "path" (name derived from filename), or "url:https://..."
///
/// # Security
/// File paths are canonicalized to prevent path traversal issues and ensure
/// consistent path representation across different working directories.
fn parse_attachments(specs: &[String]) -> Result<Vec<Attachment>> {
    let mut attachments = Vec::new();
    let cwd = std::env::current_dir().unwrap_or_default();

    for spec in specs {
        let attachment = if spec.starts_with("http://") || spec.starts_with("https://") {
            // URL attachment
            let name = spec.rsplit('/').next().unwrap_or("link");
            Attachment::url(name, spec)
        } else if let Some((name, path)) = spec.split_once(':') {
            // Named file attachment
            let full_path = cwd.join(path);
            if !full_path.exists() {
                bail!("Attachment file not found: {}", path);
            }
            // Canonicalize to resolve symlinks and normalize path
            let canonical_path = full_path
                .canonicalize()
                .with_context(|| format!("Failed to resolve path: {}", path))?;
            Attachment::file(name, canonical_path.to_string_lossy())
        } else {
            // Just a path - derive name from filename
            let path = spec;
            let full_path = cwd.join(path);
            if !full_path.exists() {
                bail!("Attachment file not found: {}", path);
            }
            // Canonicalize to resolve symlinks and normalize path
            let canonical_path = full_path
                .canonicalize()
                .with_context(|| format!("Failed to resolve path: {}", path))?;
            let name = std::path::Path::new(path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(path);
            Attachment::file(name, canonical_path.to_string_lossy())
        };
        attachments.push(attachment);
    }

    Ok(attachments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::identity::AGENT_ENV_VAR;
    use crate::core::project::{ensure_data_dir, DATA_DIR_ENV_VAR};
    use crate::storage::jsonl::read_records;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    struct TestEnv {
        _dir: TempDir,
    }

    impl TestEnv {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            unsafe {
                env::set_var(DATA_DIR_ENV_VAR, dir.path());
            }
            ensure_data_dir().unwrap();
            Self { _dir: dir }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                env::remove_var(DATA_DIR_ENV_VAR);
            }
        }
    }

    #[test]
    #[serial]
    fn test_send_to_channel() {
        let _env = TestEnv::new();

        // Use explicit agent name
        run(
            "test-general".to_string(),
            "Hello, world!".to_string(),
            None,
            vec![],
            vec![],
            Some("test-sender"),
        )
        .unwrap();

        let messages: Vec<Message> = read_records(&channel_path("test-general")).unwrap();
        assert!(!messages.is_empty());
        let last = messages.last().unwrap();
        assert_eq!(last.body, "Hello, world!");
        assert_eq!(last.agent, "test-sender");
    }

    #[test]
    #[serial]
    fn test_send_dm() {
        let _env = TestEnv::new();

        run(
            "@other-agent".to_string(),
            "Private message".to_string(),
            None,
            vec![],
            vec![],
            Some("test-sender"),
        )
        .unwrap();

        // DM channel should be created with sorted names
        let dm_path = channel_path("_dm_other-agent_test-sender");
        let messages: Vec<Message> = read_records(&dm_path).unwrap();
        assert!(!messages.is_empty());
        assert_eq!(messages.last().unwrap().body, "Private message");
    }

    #[test]
    #[serial]
    fn test_send_invalid_channel() {
        let _env = TestEnv::new();

        let result = run(
            "INVALID".to_string(),
            "test".to_string(),
            None,
            vec![],
            vec![],
            Some("test-sender"),
        );
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_send_without_identity() {
        let _env = TestEnv::new();

        // Ensure no env var
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = run(
            "general".to_string(),
            "test".to_string(),
            None,
            vec![],
            vec![],
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_send_with_labels() {
        let _env = TestEnv::new();

        run(
            "test-labeled".to_string(),
            "Bug fix ready".to_string(),
            None,
            vec!["bug".to_string(), "ready".to_string()],
            vec![],
            Some("test-sender"),
        )
        .unwrap();

        let messages: Vec<Message> = read_records(&channel_path("test-labeled")).unwrap();
        assert!(!messages.is_empty());
        assert_eq!(messages.last().unwrap().labels, vec!["bug", "ready"]);
    }
}
