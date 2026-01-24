//! Wait command - block until a relevant message arrives.

use anyhow::{Context, Result};
use chrono::DateTime;
use colored::Colorize;
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::core::identity::resolve_agent;
use crate::core::message::Message;
use crate::core::project::channels_dir;
use crate::storage::jsonl::read_records_from_offset;
use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};

pub struct WaitOptions {
    /// Wait for @mention of current agent
    pub mention: bool,
    /// Wait for messages in specific channel
    pub channel: Option<String>,
    /// Wait for messages with specific labels (any of them)
    pub labels: Vec<String>,
    /// Timeout in seconds (0 = no timeout)
    pub timeout: u64,
    /// Output as JSON
    pub json: bool,
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
}

/// Wait for a relevant message to arrive.
pub fn run(options: WaitOptions, explicit_agent: Option<&str>, project_root: &Path) -> Result<()> {
    let agent = resolve_agent(explicit_agent, project_root);

    // For --mention, we need an agent identity
    if options.mention && agent.is_none() {
        anyhow::bail!("--mention requires agent identity. Set BOTBUS_AGENT or use --agent flag.");
    }

    let channels_path = channels_dir(project_root);
    if !channels_path.exists() {
        std::fs::create_dir_all(&channels_path)?;
    }

    // Track current file offsets for all channels we're watching
    let mut channel_offsets = collect_channel_offsets(&channels_path, options.channel.as_deref())?;

    // Set up file watcher
    let (_watcher, rx) =
        watch_directory(&channels_path).with_context(|| "Failed to watch channels directory")?;

    let timeout_duration = if options.timeout > 0 {
        Some(Duration::from_secs(options.timeout))
    } else {
        None
    };

    let start = Instant::now();

    if !options.json {
        if let Some(ch) = &options.channel {
            eprint!("Waiting for messages in #{}...", ch.cyan());
        } else if options.mention {
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
        if let Some(timeout) = timeout_duration {
            if start.elapsed() >= timeout {
                let output = WaitOutput {
                    received: false,
                    message: None,
                    channel: None,
                    reason: "timeout".to_string(),
                };

                if options.json {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    println!("{} Timeout after {}s", "✗".red(), timeout.as_secs());
                }

                // Exit with code 1 on timeout
                std::process::exit(1);
            }
        }

        // Wait for file changes (with short poll interval for timeout checking)
        let poll_duration = Duration::from_millis(500);
        let changed = debounce_events(&rx, poll_duration);
        let changed_channels = filter_channel_events(changed);

        // Check each changed channel for new messages
        for channel_name in changed_channels {
            // Skip if we're filtering to a specific channel
            if let Some(ref filter_channel) = options.channel {
                if &channel_name != filter_channel {
                    continue;
                }
            }

            let channel_path = channels_path.join(format!("{}.jsonl", channel_name));
            let offset = channel_offsets.get(&channel_name).copied().unwrap_or(0);

            // Read new messages
            let (new_messages, new_offset): (Vec<Message>, u64) =
                read_records_from_offset(&channel_path, offset)?;

            // Update offset
            channel_offsets.insert(channel_name.clone(), new_offset);

            // Check each message
            for msg in new_messages {
                // Skip our own messages
                if agent.as_ref().is_some_and(|a| a == &msg.agent) {
                    continue;
                }

                // Check if message matches our filter
                let matches = if options.mention {
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
                        reason: if options.mention {
                            "mention".to_string()
                        } else {
                            "message".to_string()
                        },
                    };

                    if options.json {
                        println!("{}", serde_json::to_string_pretty(&output)?);
                    } else {
                        println!();
                        println!(
                            "{} Message received in #{}",
                            "✓".green(),
                            channel_name.cyan()
                        );
                        print_message(&msg);
                    }

                    return Ok(());
                }
            }
        }
    }
}

fn collect_channel_offsets(
    channels_path: &Path,
    filter_channel: Option<&str>,
) -> Result<std::collections::HashMap<String, u64>> {
    let mut offsets = std::collections::HashMap::new();

    if let Ok(entries) = std::fs::read_dir(channels_path) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    // Skip if filtering to specific channel
                    if let Some(filter) = filter_channel {
                        if name != filter {
                            continue;
                        }
                    }

                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    offsets.insert(name.to_string(), size);
                }
            }
        }
    }

    // If filtering to a channel that doesn't exist yet, add it with offset 0
    if let Some(filter) = filter_channel {
        offsets.entry(filter.to_string()).or_insert(0);
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
    use crate::cli::{init, send};
    use std::thread;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_collect_channel_offsets() {
        let temp = setup();

        // Send a message to create a channel
        send::run_simple(
            "general".to_string(),
            "Hello".to_string(),
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        let channels_path = channels_dir(temp.path());
        let offsets = collect_channel_offsets(&channels_path, None).unwrap();

        assert!(offsets.contains_key("general"));
        assert!(offsets["general"] > 0);
    }

    #[test]
    fn test_collect_channel_offsets_filtered() {
        let temp = setup();

        send::run_simple(
            "general".to_string(),
            "Hello".to_string(),
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        send::run_simple(
            "backend".to_string(),
            "Hello".to_string(),
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        let channels_path = channels_dir(temp.path());
        let offsets = collect_channel_offsets(&channels_path, Some("backend")).unwrap();

        // Should only have backend
        assert_eq!(offsets.len(), 1);
        assert!(offsets.contains_key("backend"));
    }

    #[test]
    fn test_wait_timeout() {
        let temp = setup();

        // This test verifies the timeout logic works
        // We can't easily test the full wait loop without threading
        let options = WaitOptions {
            mention: false,
            channel: Some("nonexistent".to_string()),
            labels: vec![],
            timeout: 1, // 1 second timeout
            json: true,
        };

        // The wait command will exit(1) on timeout, which we can't catch in a unit test
        // So we just verify the setup works
        let channels_path = channels_dir(temp.path());
        let offsets = collect_channel_offsets(&channels_path, options.channel.as_deref()).unwrap();
        assert!(offsets.contains_key("nonexistent"));
    }
}
