use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use colored::Colorize;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::core::message::Message;
use crate::core::project::{channel_path, channels_dir};
use crate::storage::jsonl::{read_last_n, read_records_from_offset};
use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};

/// Stream new messages in real-time.
pub fn run(channel: Option<String>, watch_all: bool, project_root: &Path) -> Result<()> {
    let channels = channels_dir(project_root);

    if !channels.exists() {
        println!("No channels yet. Send a message first!");
        return Ok(());
    }

    // Determine which channels to watch
    let watching: Vec<String> = if let Some(ch) = channel {
        vec![ch]
    } else if watch_all {
        list_channels(&channels)?
    } else {
        // Default to general
        vec!["general".to_string()]
    };

    if watching.is_empty() {
        println!("No channels to watch.");
        return Ok(());
    }

    // Print header
    if watching.len() == 1 {
        println!("{}", format!("Watching #{}", watching[0]).cyan().bold());
    } else {
        println!(
            "{}",
            format!("Watching {} channels", watching.len())
                .cyan()
                .bold()
        );
    }

    // Print recent context (last 10 messages per channel)
    let mut offsets: HashMap<String, u64> = HashMap::new();

    for ch in &watching {
        let path = channel_path(project_root, ch);
        if path.exists() {
            let recent: Vec<Message> = read_last_n(&path, 10).unwrap_or_default();
            for msg in &recent {
                print_message(msg, watching.len() > 1);
            }
            // Track current end of file
            offsets.insert(
                ch.clone(),
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            );
        } else {
            offsets.insert(ch.clone(), 0);
        }
    }

    println!(
        "{}",
        "--- Watching for new messages (Ctrl+C to exit) ---".dimmed()
    );

    // Set up file watcher
    let (_watcher, rx) =
        watch_directory(&channels).with_context(|| "Failed to set up file watcher")?;

    // Main watch loop
    loop {
        let changed = debounce_events(&rx, Duration::from_millis(100));
        let channel_changes = filter_channel_events(changed);

        for ch in channel_changes {
            // Only process channels we're watching
            if !watching.contains(&ch) {
                continue;
            }

            let path = channel_path(project_root, &ch);
            let offset = offsets.get(&ch).copied().unwrap_or(0);

            match read_records_from_offset::<Message>(&path, offset) {
                Ok((new_messages, new_offset)) => {
                    for msg in &new_messages {
                        print_message(msg, watching.len() > 1);
                    }
                    offsets.insert(ch, new_offset);
                }
                Err(e) => {
                    eprintln!("{}: Failed to read #{}: {}", "Error".red(), ch, e);
                }
            }
        }
    }
}

fn list_channels(channels_dir: &Path) -> Result<Vec<String>> {
    let mut channels = Vec::new();

    for entry in std::fs::read_dir(channels_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                channels.push(name.to_string());
            }
        }
    }

    Ok(channels)
}

fn print_message(msg: &Message, show_channel: bool) {
    let local_time: DateTime<Local> = msg.ts.with_timezone(&Local);
    let time_str = local_time.format("%H:%M").to_string();

    let agent_colored = colorize_agent(&msg.agent);

    if show_channel {
        println!(
            "[{}] {}{}: {}",
            time_str.dimmed(),
            format!("#{} ", msg.channel).dimmed(),
            agent_colored,
            msg.body
        );
    } else {
        println!("[{}] {}: {}", time_str.dimmed(), agent_colored, msg.body);
    }
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
    fn test_list_channels() {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path()).unwrap();
        std::fs::write(temp.path().join("general.jsonl"), "").unwrap();
        std::fs::write(temp.path().join("backend.jsonl"), "").unwrap();

        let channels = list_channels(temp.path()).unwrap();
        assert_eq!(channels.len(), 2);
    }
}
