//! Inbox command - show unread messages using stored read cursor.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::cli::history::{self, HistoryOptions, HistoryOutput};
use crate::core::identity::require_agent;
use crate::core::message::Message;
use crate::core::project::data_dir;
use crate::storage::agent_state::AgentStateManager;

pub struct InboxOptions {
    /// Channel to check (default: general)
    pub channel: Option<String>,
    /// Maximum messages to show
    pub count: usize,
    /// Auto-mark as read after displaying
    pub mark_read: bool,
    /// Output as JSON
    pub json: bool,
}

#[derive(Debug, Serialize)]
pub struct InboxOutput {
    pub channel: String,
    pub unread_count: usize,
    pub messages: Vec<Message>,
    pub next_offset: u64,
    pub marked_read: bool,
}

/// Show unread messages for the current agent.
pub fn run(options: InboxOptions, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;

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

    if options.json {
        let json_output = InboxOutput {
            channel: channel.clone(),
            unread_count: output.messages.len(),
            messages: output.messages,
            next_offset: output.next_offset,
            marked_read,
        };
        println!("{}", serde_json::to_string_pretty(&json_output)?);
        return Ok(());
    }

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
