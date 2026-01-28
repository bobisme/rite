//! Inbox command - show unread messages using stored read cursor.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;

use crate::cli::history::{self, HistoryOptions, HistoryOutput};
use crate::cli::OutputFormat;
use crate::core::identity::require_agent;
use crate::core::message::Message;
use crate::core::project::{channels_dir, data_dir};
use crate::storage::agent_state::AgentStateManager;
use crate::storage::jsonl::read_records;

pub struct InboxOptions {
    /// Channel to check (default: general)
    pub channel: Option<String>,
    /// Maximum messages to show
    pub count: usize,
    /// Auto-mark as read after displaying
    pub mark_read: bool,
    /// Output format
    pub format: OutputFormat,
    /// Check all channels for @mentions of current agent
    pub mentions: bool,
}

#[derive(Debug, Serialize)]
pub struct InboxOutput {
    pub channel: String,
    pub unread_count: usize,
    pub messages: Vec<Message>,
    pub next_offset: u64,
    pub marked_read: bool,
}

#[derive(Debug, Serialize)]
pub struct MentionedMessage {
    pub message: Message,
    pub channel: String,
}

#[derive(Debug, Serialize)]
pub struct MentionsOutput {
    pub mentions: Vec<MentionedMessage>,
    pub total_count: usize,
}

/// Show unread messages for the current agent.
pub fn run(options: InboxOptions, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;

    // Handle mentions mode: scan all channels for @mentions
    if options.mentions {
        return run_mentions_mode(&options, &agent);
    }

    // Single-channel mode (original behavior)
    let channel = options
        .channel
        .clone()
        .unwrap_or_else(|| "general".to_string());

    // Get the agent's read cursor
    let manager = AgentStateManager::new(&data_dir(), &agent);
    let cursor = manager.get_read_cursor(&channel)?;

    // Build history options using stored offset
    let history_options = HistoryOptions {
        channel: Some(channel.clone()),
        count: options.count,
        follow: false,
        timeout: None,
        follow_count: None,
        since: None,
        before: None,
        from: None,
        labels: vec![],
        after_offset: Some(cursor.offset),
        after_id: None,
        show_offset: false,
        json: false,
        agent: Some(agent.clone()),
    };

    let output: HistoryOutput = history::run_with_output(history_options)?;

    // Mark as read if requested
    let marked_read = if options.mark_read && !output.messages.is_empty() {
        manager.mark_read(&channel, output.next_offset, output.last_id.as_deref())?;
        true
    } else {
        false
    };

    // Handle output format
    match options.format {
        OutputFormat::Json => {
            let json_output = InboxOutput {
                channel: channel.clone(),
                unread_count: output.messages.len(),
                messages: output.messages,
                next_offset: output.next_offset,
                marked_read,
            };
            println!("{}", serde_json::to_string_pretty(&json_output)?);
        }
        OutputFormat::Toon => {
            // TOON format: minimal, structured text
            println!("channel: {}", channel);
            println!("unread_count: {}", output.messages.len());
            println!("next_offset: {}", output.next_offset);
            println!("marked_read: {}", marked_read);
            println!();
            println!("messages:");
            for msg in &output.messages {
                println!("  - id: {}", msg.id);
                println!("    agent: {}", msg.agent);
                println!("    ts: {}", msg.ts.to_rfc3339());
                println!("    body: {}", msg.body);
            }
        }
        OutputFormat::Text => {
            // Human-readable text output
            if output.messages.is_empty() {
                println!("{} No unread messages in #{}", "✓".green(), channel.cyan());
                return Ok(());
            }

            // Print header with unread count
            println!(
                "{} {} unread message{} in #{}",
                "→".cyan(),
                output.messages.len(),
                if output.messages.len() == 1 { "" } else { "s" },
                channel.cyan().bold()
            );
            println!();

            // Print messages
            for msg in &output.messages {
                print_message(msg);
            }

            if marked_read {
                println!();
                println!(
                    "{} Marked as read (offset: {})",
                    "✓".green(),
                    output.next_offset
                );
            } else {
                // Show hint about marking as read
                println!();
                println!(
                    "{} Run 'botbus inbox {} --mark-read' or 'botbus mark-read {}' to mark as read",
                    "Tip:".dimmed(),
                    channel,
                    channel
                );
            }
        }
    }

    Ok(())
}

/// Scan all channels for messages mentioning the current agent.
fn run_mentions_mode(options: &InboxOptions, agent: &str) -> Result<()> {
    let channels_path = channels_dir();

    if !channels_path.exists() {
        match options.format {
            OutputFormat::Json => {
                let output = MentionsOutput {
                    mentions: vec![],
                    total_count: 0,
                };
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
            OutputFormat::Toon => {
                println!("total_count: 0");
                println!();
                println!("mentions: []");
            }
            OutputFormat::Text => {
                println!("{} No mentions found", "✓".green());
            }
        }
        return Ok(());
    }

    // Read all channel files
    let entries: Vec<_> = std::fs::read_dir(&channels_path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    // Collect all messages that mention this agent
    let mut all_mentions: Vec<MentionedMessage> = Vec::new();

    for entry in entries {
        let path = entry.path();
        let channel_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Read all messages from this channel
        let messages: Vec<Message> = read_records(&path).unwrap_or_default();

        // Filter for messages mentioning this agent (case-sensitive)
        for msg in messages {
            // Check if agent is in the mentions field
            if msg.mentions.iter().any(|m| m == agent) {
                all_mentions.push(MentionedMessage {
                    message: msg,
                    channel: channel_name.clone(),
                });
            }
        }
    }

    // Sort by timestamp (most recent first)
    all_mentions.sort_by(|a, b| b.message.ts.cmp(&a.message.ts));

    // Apply count limit
    let limited_mentions: Vec<_> = all_mentions
        .into_iter()
        .take(options.count)
        .collect();

    let total_count = limited_mentions.len();

    // Handle output format
    match options.format {
        OutputFormat::Json => {
            let output = MentionsOutput {
                mentions: limited_mentions,
                total_count,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Toon => {
            println!("total_count: {}", total_count);
            println!();
            println!("mentions:");
            for mention in &limited_mentions {
                println!("  - channel: {}", mention.channel);
                println!("    id: {}", mention.message.id);
                println!("    agent: {}", mention.message.agent);
                println!("    ts: {}", mention.message.ts.to_rfc3339());
                println!("    body: {}", mention.message.body);
            }
        }
        OutputFormat::Text => {
            if limited_mentions.is_empty() {
                println!("{} No mentions found", "✓".green());
                return Ok(());
            }

            // Print header
            println!(
                "{} {} mention{} of @{}",
                "→".cyan(),
                total_count,
                if total_count == 1 { "" } else { "s" },
                agent.yellow().bold()
            );
            println!();

            // Group by channel for cleaner display
            let mut by_channel: HashMap<String, Vec<&MentionedMessage>> = HashMap::new();
            for mention in &limited_mentions {
                by_channel
                    .entry(mention.channel.clone())
                    .or_default()
                    .push(mention);
            }

            // Sort channels alphabetically
            let mut channels: Vec<_> = by_channel.keys().cloned().collect();
            channels.sort();

            for channel in channels {
                let mentions = by_channel.get(&channel).unwrap();
                println!("{}", format!("#{}", channel).cyan().bold());
                for mention in mentions {
                    print_message(&mention.message);
                }
                println!();
            }
        }
    }

    Ok(())
}

fn print_message(msg: &Message) {
    use chrono::{DateTime, Local};

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
    // Integration tests moved to tests/integration/ since they require
    // global data directory mocking
}
