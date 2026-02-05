//! Message retrieval by ID.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local};
use colored::Colorize;
use serde::Serialize;

use super::OutputFormat;
use super::format::to_toon;
use crate::core::message::Message;
use crate::core::project::channels_dir;
use crate::storage::jsonl::read_records;

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
        }
    }
}

/// Get a message by its ULID ID.
///
/// Searches all channels for the message with the given ID.
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

    for entry in entries {
        let path = entry.path();
        let messages: Vec<Message> = read_records(&path).unwrap_or_default();

        for msg in messages {
            if msg.id.to_string() == id {
                // Found it!
                let output = MessageOutput::from(&msg);
                return print_message(&output, &msg, format);
            }
        }
    }

    Err(anyhow!("Message not found: {}", id))
}

fn print_message(output: &MessageOutput, msg: &Message, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(output)?);
        }
        OutputFormat::Toon => {
            println!("{}", to_toon(output));
        }
        OutputFormat::Text => {
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
