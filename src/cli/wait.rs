//! Wait command - block until a relevant message arrives.

use anyhow::{Context, Result};
use chrono::DateTime;
use colored::Colorize;
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::cli::OutputFormat;
use crate::core::identity::resolve_agent;
use crate::core::message::{Message, read_messages_from_offset};
use crate::core::project::channels_dir;
use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};

pub struct WaitOptions {
    /// Wait for @mentions of current agent from any channel
    pub mentions: bool,
    /// Wait for messages in specific channel(s)
    pub channels: Vec<String>,
    /// Wait for messages with specific labels (any of them)
    pub labels: Vec<String>,
    /// Timeout in seconds (0 = no timeout)
    pub timeout: u64,
    /// Output format
    pub format: OutputFormat,
}

#[derive(Debug, Serialize)]
pub struct WaitOutput {
    /// Whether a message was received (vs timeout)
    pub received: bool,
    /// The triggering message (if received)
    pub message: Option<Message>,
    /// Channel the message was in
    pub channel: Option<String>,
    /// Reason for returning
    pub reason: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<String>,
}

/// Wait for a relevant message to arrive.
pub fn run(mut options: WaitOptions, explicit_agent: Option<&str>) -> Result<()> {
    let agent = resolve_agent(explicit_agent);

    // For --mentions, we need an agent identity
    if options.mentions && agent.is_none() {
        anyhow::bail!("--mentions requires agent identity. Set BOTBUS_AGENT or use --agent flag.");
    }

    // Strip # prefix from channels if present (common user pattern)
    options.channels = options
        .channels
        .iter()
        .map(|ch| ch.strip_prefix('#').unwrap_or(ch).to_string())
        .collect();

    let filter_channels: Option<Vec<&str>> = if options.channels.is_empty() {
        None
    } else {
        Some(options.channels.iter().map(|s| s.as_str()).collect())
    };

    let channels_path = channels_dir();
    if !channels_path.exists() {
        std::fs::create_dir_all(&channels_path)?;
    }

    // Track current file offsets for all channels we're watching
    let mut channel_offsets = collect_channel_offsets(&channels_path, filter_channels.as_deref())?;

    // Set up file watcher
    let (_watcher, rx) =
        watch_directory(&channels_path).with_context(|| "Failed to watch channels directory")?;

    let timeout_duration = if options.timeout > 0 {
        Some(Duration::from_secs(options.timeout))
    } else {
        None
    };

    let start = Instant::now();

    if options.format != OutputFormat::Json {
        if !options.channels.is_empty() {
            let ch_display: Vec<String> =
                options.channels.iter().map(|c| format!("#{}", c)).collect();
            eprint!(
                "Waiting for messages in {}...",
                ch_display.join(", ").cyan()
            );
        } else if options.mentions {
            eprint!("Waiting for @{}...", agent.as_ref().unwrap().cyan());
        } else if !options.labels.is_empty() {
            eprint!("Waiting for messages with labels {:?}...", options.labels);
        } else {
            eprint!("Waiting for any message...");
        }
        if let Some(t) = timeout_duration {
            eprintln!(" (timeout: {}s)", t.as_secs());
        } else {
            eprintln!();
        }
    }

    loop {
        // Check timeout
        if let Some(timeout) = timeout_duration
            && start.elapsed() >= timeout
        {
            let output = WaitOutput {
                received: false,
                message: None,
                channel: None,
                reason: "timeout".to_string(),
                advice: vec![],
            };

            match options.format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Pretty => {
                    println!("{} Timeout after {}s", "✗".red(), timeout.as_secs());
                }
                OutputFormat::Text => {
                    println!("timeout");
                }
            }

            // Exit with code 1 on timeout
            std::process::exit(1);
        }

        // Wait for file changes (with short poll interval for timeout checking)
        let poll_duration = Duration::from_millis(500);
        let changed = debounce_events(&rx, poll_duration);
        let changed_channels = filter_channel_events(changed);

        // Check each changed channel for new messages
        for channel_name in changed_channels {
            // Skip if we're filtering to specific channels
            if let Some(ref filter) = filter_channels
                && !filter.contains(&channel_name.as_str())
            {
                continue;
            }

            let channel_path = channels_path.join(format!("{}.jsonl", channel_name));
            let offset = channel_offsets.get(&channel_name).copied().unwrap_or(0);

            // Read new messages
            let (new_messages, new_offset): (Vec<Message>, u64) =
                read_messages_from_offset(&channel_path, offset)?;

            // Update offset
            channel_offsets.insert(channel_name.clone(), new_offset);

            // Check each message
            for msg in new_messages {
                // Skip our own messages
                if agent.as_ref().is_some_and(|a| a == &msg.agent) {
                    continue;
                }

                // Check if message matches our filter
                let matches = if options.mentions {
                    // Check for @mention in body
                    let mention = format!("@{}", agent.as_ref().unwrap());
                    msg.body.contains(&mention)
                } else if !options.labels.is_empty() {
                    // Check for matching labels
                    msg.has_any_label(&options.labels)
                } else {
                    // Any message matches
                    true
                };

                if matches {
                    let output = WaitOutput {
                        received: true,
                        message: Some(msg.clone()),
                        channel: Some(channel_name.clone()),
                        reason: if options.mentions {
                            "mention".to_string()
                        } else {
                            "message".to_string()
                        },
                        advice: vec![],
                    };

                    match options.format {
                        OutputFormat::Json => {
                            println!("{}", serde_json::to_string_pretty(&output)?);
                        }
                        OutputFormat::Pretty => {
                            println!();
                            println!(
                                "{} Message received in #{}",
                                "✓".green(),
                                channel_name.cyan()
                            );
                            print_message(&msg);
                        }
                        OutputFormat::Text => {
                            println!("{}  {}  {}", channel_name, msg.agent, msg.body);
                        }
                    }

                    return Ok(());
                }
            }
        }
    }
}

fn collect_channel_offsets(
    channels_path: &Path,
    filter_channels: Option<&[&str]>,
) -> Result<std::collections::HashMap<String, u64>> {
    let mut offsets = std::collections::HashMap::new();

    if let Ok(entries) = std::fs::read_dir(channels_path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                // Skip if filtering to specific channels
                if let Some(filters) = filter_channels
                    && !filters.contains(&name)
                {
                    continue;
                }

                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                offsets.insert(name.to_string(), size);
            }
        }
    }

    // If filtering to channels that don't exist yet, add them with offset 0
    if let Some(filters) = filter_channels {
        for filter in filters {
            offsets.entry(filter.to_string()).or_insert(0);
        }
    }

    Ok(offsets)
}

fn print_message(msg: &Message) {
    use chrono::Local;

    let local_time: DateTime<Local> = msg.ts.with_timezone(&Local);
    let time_str = local_time.format("%H:%M").to_string();

    let agent_colored = colorize_agent(&msg.agent);

    println!("[{}] {}: {}", time_str.dimmed(), agent_colored, msg.body);
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

    use tempfile::TempDir;

    #[test]
    fn test_collect_channel_offsets_filtered() {
        // This test only tests the offset collection logic, not the full wait
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path()).unwrap();
        std::fs::write(temp.path().join("general.jsonl"), "{}\n").unwrap();
        std::fs::write(temp.path().join("backend.jsonl"), "{}\n").unwrap();

        let offsets = collect_channel_offsets(temp.path(), Some(&["backend"])).unwrap();

        // Should only have backend
        assert_eq!(offsets.len(), 1);
        assert!(offsets.contains_key("backend"));
    }

    #[test]
    fn test_collect_channel_offsets_multiple() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path()).unwrap();
        std::fs::write(temp.path().join("general.jsonl"), "{}\n").unwrap();
        std::fs::write(temp.path().join("backend.jsonl"), "{}\n").unwrap();
        std::fs::write(temp.path().join("frontend.jsonl"), "{}\n").unwrap();

        let offsets = collect_channel_offsets(temp.path(), Some(&["backend", "frontend"])).unwrap();

        assert_eq!(offsets.len(), 2);
        assert!(offsets.contains_key("backend"));
        assert!(offsets.contains_key("frontend"));
    }

    #[test]
    fn test_collect_channel_offsets_all() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path()).unwrap();
        std::fs::write(temp.path().join("general.jsonl"), "{}\n").unwrap();
        std::fs::write(temp.path().join("backend.jsonl"), "{}\n").unwrap();

        let offsets = collect_channel_offsets(temp.path(), None).unwrap();

        assert_eq!(offsets.len(), 2);
        assert!(offsets.contains_key("general"));
        assert!(offsets.contains_key("backend"));
    }
}
