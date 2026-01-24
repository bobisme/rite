//! Inbox command - show unread messages using stored read cursor.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::cli::history::{self, HistoryOptions, HistoryOutput};
use crate::core::identity::resolve_agent;
use crate::storage::agent_state::AgentStateManager;

pub struct InboxOptions {
    /// Channel to check (default: general)
    pub channel: Option<String>,
    /// Maximum messages to show
    pub count: usize,
    /// Auto-mark as read after displaying
    pub mark_read: bool,
}

/// Show unread messages for the current agent.
pub fn run(options: InboxOptions, explicit_agent: Option<&str>, project_root: &Path) -> Result<()> {
    let agent = resolve_agent(explicit_agent, project_root)
        .context("Could not determine agent identity")?;

    let channel = options
        .channel
        .clone()
        .unwrap_or_else(|| "general".to_string());

    // Get the agent's read cursor
    let manager = AgentStateManager::new(project_root, &agent);
    let cursor = manager.get_read_cursor(&channel)?;

    // Build history options using stored offset
    let history_options = HistoryOptions {
        channel: Some(channel.clone()),
        count: options.count,
        follow: false,
        since: None,
        before: None,
        from: None,
        after_offset: Some(cursor.offset),
        after_id: None,
        show_offset: false,
        json: false, // inbox handles its own output
    };

    let output: HistoryOutput = history::run_with_output(history_options, project_root)?;

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

    // Auto mark as read if requested
    if options.mark_read {
        manager.mark_read(&channel, output.next_offset, output.last_id.as_deref())?;
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

fn print_message(msg: &crate::core::message::Message) {
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
    use super::*;
    use crate::cli::{init, send};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_inbox_empty() {
        let temp = setup();

        let options = InboxOptions {
            channel: Some("general".to_string()),
            count: 50,
            mark_read: false,
        };
        run(options, Some("TestAgent"), temp.path()).unwrap();
    }

    #[test]
    fn test_inbox_with_unread() {
        let temp = setup();

        // Send a message
        send::run(
            "general".to_string(),
            "Hello!".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        // Check inbox (should show 1 unread)
        let options = InboxOptions {
            channel: Some("general".to_string()),
            count: 50,
            mark_read: false,
        };
        run(options, Some("Reader"), temp.path()).unwrap();
    }

    #[test]
    fn test_inbox_mark_read() {
        let temp = setup();

        // Send a message
        send::run(
            "general".to_string(),
            "Hello!".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        // Check inbox and mark as read
        let options = InboxOptions {
            channel: Some("general".to_string()),
            count: 50,
            mark_read: true,
        };
        run(options, Some("Reader"), temp.path()).unwrap();

        // Check inbox again (should be empty)
        let options2 = InboxOptions {
            channel: Some("general".to_string()),
            count: 50,
            mark_read: false,
        };
        // This should succeed and show no unread
        run(options2, Some("Reader"), temp.path()).unwrap();
    }

    #[test]
    fn test_inbox_per_agent_isolation() {
        let temp = setup();

        // Send a message
        send::run(
            "general".to_string(),
            "Hello!".to_string(),
            None,
            Some("Sender"),
            temp.path(),
        )
        .unwrap();

        // Agent1 marks as read
        let options = InboxOptions {
            channel: Some("general".to_string()),
            count: 50,
            mark_read: true,
        };
        run(options, Some("Agent1"), temp.path()).unwrap();

        // Agent2 should still see it as unread
        // (This test just verifies no crash; proper assertion would need output capture)
        let options2 = InboxOptions {
            channel: Some("general".to_string()),
            count: 50,
            mark_read: false,
        };
        run(options2, Some("Agent2"), temp.path()).unwrap();
    }
}
