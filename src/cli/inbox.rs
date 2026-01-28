//! Inbox command - show unread messages using stored read cursor.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;

use crate::cli::OutputFormat;
use crate::cli::history::{self, HistoryOptions, HistoryOutput};
use crate::core::channel::{dm_agents, is_dm_channel};
use crate::core::identity::require_agent;
use crate::core::message::Message;
use crate::core::project::{channels_dir, data_dir};
use crate::storage::agent_state::AgentStateManager;
use crate::storage::jsonl::read_records;

pub struct InboxOptions {
    /// Specific channels to check (if empty, checks general + DMs)
    pub channels: Vec<String>,
    /// Maximum messages to show per channel
    pub count: usize,
    /// Auto-mark as read after displaying
    pub mark_read: bool,
    /// Output format
    pub format: OutputFormat,
    /// Include all channels (not just general + DMs)
    pub all: bool,
    /// Check all channels for @mentions of current agent
    pub mentions: bool,
    /// Only show the count of unread messages
    pub count_only: bool,
}

#[derive(Debug, Serialize)]
pub struct ChannelInbox {
    pub channel: String,
    pub is_dm: bool,
    pub unread_count: usize,
    pub messages: Vec<Message>,
    pub next_offset: u64,
    pub marked_read: bool,
}

#[derive(Debug, Serialize)]
pub struct InboxOutput {
    pub total_unread: usize,
    pub channels: Vec<ChannelInbox>,
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

    // Get the agent's state manager
    let manager = AgentStateManager::new(&data_dir(), &agent);

    // Determine which channels to check
    let channels_to_check = if !options.channels.is_empty() {
        // User specified explicit channels
        options.channels.clone()
    } else if options.all {
        // Get all channels
        get_all_channels()?
    } else {
        // Default: general + DM channels for this agent
        let mut channels = vec!["general".to_string()];
        channels.extend(get_dm_channels_for_agent(&agent)?);
        channels
    };

    // Collect inbox data for each channel
    let mut channel_inboxes: Vec<ChannelInbox> = Vec::new();
    let mut total_unread = 0;

    for channel in channels_to_check {
        let cursor = manager.get_read_cursor(&channel)?;

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

        // Skip channels with no unread messages
        if output.messages.is_empty() {
            continue;
        }

        let unread_count = output.messages.len();
        total_unread += unread_count;

        // Mark as read if requested
        let marked_read = if options.mark_read {
            manager.mark_read(&channel, output.next_offset, output.last_id.as_deref())?;
            true
        } else {
            false
        };

        channel_inboxes.push(ChannelInbox {
            channel: channel.clone(),
            is_dm: is_dm_channel(&channel),
            unread_count,
            messages: output.messages,
            next_offset: output.next_offset,
            marked_read,
        });
    }

    // Handle count-only mode
    if options.count_only {
        println!("{}", total_unread);
        return Ok(());
    }

    // Handle output format
    match options.format {
        OutputFormat::Json => {
            let json_output = InboxOutput {
                total_unread,
                channels: channel_inboxes,
            };
            println!("{}", serde_json::to_string_pretty(&json_output)?);
        }
        OutputFormat::Toon => {
            println!("total_unread: {}", total_unread);
            println!("channel_count: {}", channel_inboxes.len());
            println!();
            for inbox in &channel_inboxes {
                println!("channel: {}", inbox.channel);
                println!("  is_dm: {}", inbox.is_dm);
                println!("  unread_count: {}", inbox.unread_count);
                println!("  next_offset: {}", inbox.next_offset);
                println!("  marked_read: {}", inbox.marked_read);
                println!("  messages:");
                for msg in &inbox.messages {
                    println!("    - id: {}", msg.id);
                    println!("      agent: {}", msg.agent);
                    println!("      ts: {}", msg.ts.to_rfc3339());
                    println!("      body: {}", msg.body);
                }
                println!();
            }
        }
        OutputFormat::Text => {
            if channel_inboxes.is_empty() {
                println!("{} No unread messages", "✓".green());
                return Ok(());
            }

            // Print summary
            println!(
                "{} {} unread message{} across {} channel{}",
                "→".cyan(),
                total_unread,
                if total_unread == 1 { "" } else { "s" },
                channel_inboxes.len(),
                if channel_inboxes.len() == 1 { "" } else { "s" }
            );
            println!();

            // Print messages grouped by channel
            for inbox in &channel_inboxes {
                print_channel_inbox(inbox, &agent);
            }

            if options.mark_read {
                println!();
                println!("{} Marked all as read", "✓".green());
            } else {
                println!();
                println!(
                    "{} Run 'botbus inbox --mark-read' to mark all as read",
                    "Tip:".dimmed()
                );
            }
        }
    }

    Ok(())
}

/// Get all DM channels that involve the specified agent.
fn get_dm_channels_for_agent(agent: &str) -> Result<Vec<String>> {
    let channels_path = channels_dir();
    if !channels_path.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&channels_path)?;
    let mut dm_channels = Vec::new();

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }

        let channel_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if is_dm_channel(&channel_name)
            && let Some((a, b)) = dm_agents(&channel_name)
                && (a == agent || b == agent) {
                    dm_channels.push(channel_name);
                }
    }

    Ok(dm_channels)
}

/// Get all channels (for --all flag).
fn get_all_channels() -> Result<Vec<String>> {
    let channels_path = channels_dir();
    if !channels_path.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&channels_path)?;
    let mut all_channels = Vec::new();

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }

        let channel_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        all_channels.push(channel_name);
    }

    Ok(all_channels)
}

/// Print a channel inbox in human-readable format.
fn print_channel_inbox(inbox: &ChannelInbox, current_agent: &str) {
    // Format channel name
    let channel_display = if inbox.is_dm {
        // For DMs, show the other participant's name
        if let Some((a, b)) = dm_agents(&inbox.channel) {
            let other = if a == current_agent { b } else { a };
            format!("@{}", other)
        } else {
            inbox.channel.clone()
        }
    } else {
        format!("#{}", inbox.channel)
    };

    println!(
        "{} {} message{}",
        channel_display.cyan().bold(),
        inbox.unread_count,
        if inbox.unread_count == 1 { "" } else { "s" }
    );

    for msg in &inbox.messages {
        print_message(msg);
    }
    println!();
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
    let limited_mentions: Vec<_> = all_mentions.into_iter().take(options.count).collect();

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
