//! Message retrieval and deletion by ID.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, Utc};
use colored::Colorize;
use serde::Serialize;

use super::OutputFormat;
use crate::core::identity::require_agent;
use crate::core::message::{Message, MessageMeta};
use crate::core::project::channels_dir;
use crate::storage::jsonl::{append_record, read_records};

/// Output for a single message retrieval.
#[derive(Debug, Serialize)]
pub struct MessageOutput {
    pub id: String,
    pub agent: String,
    pub channel: String,
    pub body: String,
    pub ts: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentOutput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<String>,
}

/// Simplified attachment output.
#[derive(Debug, Serialize)]
pub struct AttachmentOutput {
    pub name: String,
    #[serde(flatten)]
    pub content: serde_json::Value,
}

impl From<&Message> for MessageOutput {
    fn from(msg: &Message) -> Self {
        let attachments: Vec<AttachmentOutput> = msg
            .attachments
            .iter()
            .map(|a| AttachmentOutput {
                name: a.name.clone(),
                content: serde_json::to_value(&a.content).unwrap_or(serde_json::Value::Null),
            })
            .collect();

        Self {
            id: msg.id.to_string(),
            agent: msg.agent.clone(),
            channel: msg.channel.clone(),
            body: msg.body.clone(),
            ts: msg.ts.to_rfc3339(),
            labels: msg.labels.clone(),
            mentions: msg.mentions.clone(),
            attachments,
            advice: vec![], // Informational command, no specific next action
        }
    }
}

/// Get a message by its ULID ID.
///
/// Searches all channels for the message with the given ID.
/// If the message has been deleted, shows deletion metadata instead.
pub fn get(id: &str, format: OutputFormat) -> Result<()> {
    let channels_path = channels_dir();

    if !channels_path.exists() {
        return Err(anyhow!("Message not found: {}", id));
    }

    // Scan all channel files for the message
    let entries: Vec<_> = std::fs::read_dir(&channels_path)
        .with_context(|| "Failed to read channels directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    // Try to parse as ULID for tombstone checking (optional — invalid IDs just won't match)
    let target_ulid: Option<ulid::Ulid> = id.parse().ok();

    let mut tombstone: Option<Message> = None;
    let mut original: Option<Message> = None;

    for entry in &entries {
        let path = entry.path();
        let messages: Vec<Message> = read_records(&path).unwrap_or_default();

        for msg in &messages {
            if msg.id.to_string() == id {
                original = Some(msg.clone());
            }
            if let Some(target) = target_ulid
                && let Some(MessageMeta::Deleted { target_id, .. }) = &msg.meta
                && *target_id == target
            {
                tombstone = Some(msg.clone());
            }
        }
    }

    // If there's a tombstone, show deletion info
    if let Some(ref tombstone_msg) = tombstone
        && let Some(MessageMeta::Deleted {
            deleted_by,
            deleted_at,
            ..
        }) = &tombstone_msg.meta
    {
        match format {
            OutputFormat::Json => {
                let output = serde_json::json!({
                    "id": id,
                    "deleted": true,
                    "deleted_by": deleted_by,
                    "deleted_at": deleted_at.to_rfc3339(),
                    "original_agent": original.as_ref().map(|m| &m.agent),
                    "original_channel": original.as_ref().map(|m| &m.channel),
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
            OutputFormat::Pretty => {
                let local_time: DateTime<Local> = deleted_at.with_timezone(&Local);
                let time_str = local_time.format("%Y-%m-%d %H:%M:%S").to_string();

                println!("{}: {}", "ID".dimmed(), id.cyan());
                println!("{}: {}", "Status".dimmed(), "DELETED".red().bold());
                println!("{}: {}", "Deleted by".dimmed(), deleted_by);
                println!("{}: {}", "Deleted at".dimmed(), time_str);
                if let Some(ref orig) = original {
                    println!("{}: #{}", "Channel".dimmed(), orig.channel.cyan());
                    println!(
                        "{}: {}",
                        "Original author".dimmed(),
                        colorize_agent(&orig.agent)
                    );
                }
            }
            OutputFormat::Text => {
                let time_str = deleted_at.to_rfc3339();
                if let Some(ref orig) = original {
                    println!(
                        "{}  {}  {}  [deleted by {} at {}]",
                        id, orig.agent, orig.channel, deleted_by, time_str
                    );
                } else {
                    println!("{}  [deleted by {} at {}]", id, deleted_by, time_str);
                }
            }
        }
        return Ok(());
    }

    // No tombstone — show normally if original exists
    if let Some(ref msg) = original {
        let output = MessageOutput::from(msg);
        return print_message(&output, msg, format);
    }

    Err(anyhow!("Message not found: {}", id))
}

/// Delete a message by appending a tombstone record.
///
/// Searches all channels for the message, prompts for confirmation,
/// then appends a tombstone to the same channel file.
pub fn delete(id: &str, skip_confirm: bool, explicit_agent: Option<&str>) -> Result<()> {
    use std::io::{self, Write};

    let agent = require_agent(explicit_agent)?;
    let channels_path = channels_dir();

    if !channels_path.exists() {
        return Err(anyhow!("Message not found: {}", id));
    }

    let target_ulid: ulid::Ulid = id
        .parse()
        .map_err(|_| anyhow!("Invalid message ID: {}", id))?;

    // Find the message and its channel file
    let entries: Vec<_> = std::fs::read_dir(&channels_path)
        .with_context(|| "Failed to read channels directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    let mut found_msg: Option<Message> = None;
    let mut found_path: Option<std::path::PathBuf> = None;

    for entry in &entries {
        let path = entry.path();
        let messages: Vec<Message> = read_records(&path).unwrap_or_default();

        // Check if already deleted
        for msg in &messages {
            if let Some(MessageMeta::Deleted { target_id, .. }) = &msg.meta
                && *target_id == target_ulid
            {
                return Err(anyhow!("Message {} is already deleted", id));
            }
        }

        for msg in messages {
            if msg.id == target_ulid {
                found_msg = Some(msg);
                found_path = Some(path.clone());
                break;
            }
        }

        if found_msg.is_some() {
            break;
        }
    }

    let msg = found_msg.ok_or_else(|| anyhow!("Message not found: {}", id))?;
    let channel_file = found_path.unwrap();

    // Show what will be deleted
    eprintln!();
    eprintln!("{}", "WARNING".bold().red());
    eprintln!("This will delete the following message (append-only tombstone):");
    eprintln!();
    eprintln!("  {}: {}", "ID".dimmed(), msg.id.to_string().cyan());
    eprintln!("  {}: #{}", "Channel".dimmed(), msg.channel.cyan());
    eprintln!("  {}: {}", "From".dimmed(), colorize_agent(&msg.agent));
    eprintln!("  {}: {}", "Body".dimmed(), msg.body);
    eprintln!();

    // Confirm deletion
    if !skip_confirm {
        eprint!("Type {} to confirm: ", format!("delete {}", id).bold());
        io::stderr().flush()?;

        let mut confirmation = String::new();
        io::stdin()
            .read_line(&mut confirmation)
            .context("Failed to read confirmation")?;

        let expected = format!("delete {}", id);
        if confirmation.trim() != expected {
            anyhow::bail!("Confirmation did not match. Deletion aborted.");
        }
    }

    // Append tombstone
    let tombstone =
        Message::new(&agent, &msg.channel, "[message deleted]").with_meta(MessageMeta::Deleted {
            target_id: target_ulid,
            deleted_by: agent.clone(),
            deleted_at: Utc::now(),
        });

    append_record(&channel_file, &tombstone)?;

    eprintln!("{} Message {} deleted", "Done:".green(), id);

    Ok(())
}

fn print_message(output: &MessageOutput, msg: &Message, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Pretty => {
            let local_time: DateTime<Local> = msg.ts.with_timezone(&Local);
            let time_str = local_time.format("%Y-%m-%d %H:%M:%S").to_string();

            println!("{}: {}", "ID".dimmed(), output.id.cyan());
            println!("{}: #{}", "Channel".dimmed(), output.channel.cyan());
            println!("{}: {}", "From".dimmed(), colorize_agent(&output.agent));
            println!("{}: {}", "Time".dimmed(), time_str);

            if !output.labels.is_empty() {
                let labels_str = output
                    .labels
                    .iter()
                    .map(|l| format!("[{}]", l).yellow().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("{}: {}", "Labels".dimmed(), labels_str);
            }

            if !output.mentions.is_empty() {
                let mentions_str = output
                    .mentions
                    .iter()
                    .map(|m| format!("@{}", m))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("{}: {}", "Mentions".dimmed(), mentions_str);
            }

            if !output.attachments.is_empty() {
                println!(
                    "{}: {} attachment(s)",
                    "Attachments".dimmed(),
                    output.attachments.len()
                );
                for attach in &msg.attachments {
                    if attach.is_available() {
                        println!("  - {}", attach.name);
                    } else {
                        println!("  - {} {}", attach.name, "(not available locally)".dimmed());
                    }
                }
            }

            println!();
            println!("{}", output.body);
        }
        OutputFormat::Text => {
            let time_str = msg.ts.to_rfc3339();
            println!(
                "{}  {}  {}  {}  {}",
                output.id, output.agent, output.channel, time_str, output.body
            );
        }
    }

    Ok(())
}

fn colorize_agent(name: &str) -> colored::ColoredString {
    let hash: usize = name.bytes().map(|b| b as usize).sum();
    let colors = [
        colored::Color::Cyan,
        colored::Color::Green,
        colored::Color::Yellow,
        colored::Color::Blue,
        colored::Color::Magenta,
    ];
    let color = colors[hash % colors.len()];
    name.color(color).bold()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::message::Message;
    use crate::core::project::{DATA_DIR_ENV_VAR, channel_path, ensure_data_dir};
    use crate::storage::jsonl::append_record;
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
    fn test_get_nonexistent_message() {
        let _env = TestEnv::new();
        let result = get("01ABCDEFGHIJKLMNOP123456", OutputFormat::Text);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    #[serial]
    fn test_get_existing_message() {
        let _env = TestEnv::new();

        // Create a message directly
        let msg = Message::new("test-agent", "test-messages", "Test message body");
        let msg_id = msg.id.to_string();
        let path = channel_path("test-messages");
        append_record(&path, &msg).unwrap();

        // Now retrieve it
        let result = get(&msg_id, OutputFormat::Json);
        assert!(result.is_ok());
    }
}
