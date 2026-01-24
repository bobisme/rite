use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use colored::Colorize;
use std::path::Path;

use crate::core::message::Message;
use crate::core::project::channel_path;
use crate::storage::jsonl::{read_last_n, read_records};

pub struct HistoryOptions {
    pub channel: Option<String>,
    pub count: usize,
    pub follow: bool,
    pub since: Option<String>,
    pub before: Option<String>,
    pub from: Option<String>,
}

/// View message history.
pub fn run(options: HistoryOptions, project_root: &Path) -> Result<()> {
    let channel = options
        .channel
        .clone()
        .unwrap_or_else(|| "general".to_string());
    let path = channel_path(project_root, &channel);

    if !path.exists() {
        println!("Channel #{} has no messages yet.", channel);
        return Ok(());
    }

    // Read messages
    let messages: Vec<Message> =
        if options.since.is_some() || options.before.is_some() || options.from.is_some() {
            // Need to filter, read all and filter
            let all: Vec<Message> = read_records(&path)
                .with_context(|| format!("Failed to read channel #{}", channel))?;
            filter_messages(all, &options)
        } else {
            // Just get last N
            read_last_n(&path, options.count)
                .with_context(|| format!("Failed to read channel #{}", channel))?
        };

    if messages.is_empty() {
        println!("No messages match your criteria.");
        return Ok(());
    }

    // Print header
    println!("{}", format!("#{}", channel).cyan().bold());

    // Print messages
    for msg in &messages {
        print_message(msg);
    }

    // Follow mode
    if options.follow {
        follow_channel(&path, project_root)?;
    }

    Ok(())
}

fn filter_messages(messages: Vec<Message>, options: &HistoryOptions) -> Vec<Message> {
    let mut filtered: Vec<Message> = messages
        .into_iter()
        .filter(|msg| {
            // Filter by sender
            if let Some(from) = &options.from {
                if &msg.agent != from {
                    return false;
                }
            }

            // Filter by since
            if let Some(since_str) = &options.since {
                if let Ok(since) = parse_datetime(since_str) {
                    if msg.ts < since {
                        return false;
                    }
                }
            }

            // Filter by before
            if let Some(before_str) = &options.before {
                if let Ok(before) = parse_datetime(before_str) {
                    if msg.ts > before {
                        return false;
                    }
                }
            }

            true
        })
        .collect();

    // Limit to count (take last N after filtering)
    let start = filtered.len().saturating_sub(options.count);
    filtered.drain(..start);
    filtered
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>> {
    // Try parsing as RFC3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try parsing as just a date
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).unwrap();
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    // Try parsing as date + time
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    anyhow::bail!("Could not parse datetime: {}", s)
}

fn print_message(msg: &Message) {
    let local_time: DateTime<Local> = msg.ts.with_timezone(&Local);
    let time_str = local_time.format("%H:%M").to_string();

    // Color the agent name consistently
    let agent_colored = colorize_agent(&msg.agent);

    println!("[{}] {}: {}", time_str.dimmed(), agent_colored, msg.body);
}

fn colorize_agent(name: &str) -> colored::ColoredString {
    // Simple hash to pick a color
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

fn follow_channel(path: &Path, project_root: &Path) -> Result<()> {
    use crate::core::project::channels_dir;
    use crate::storage::jsonl::read_records_from_offset;
    use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};
    use std::time::Duration;

    println!("{}", "--- Following (Ctrl+C to exit) ---".dimmed());

    let channels = channels_dir(project_root);
    let (_watcher, rx) = watch_directory(&channels)?;

    // Track our position in the file
    let mut offset = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    loop {
        let changed = debounce_events(&rx, Duration::from_millis(100));
        let channel_changes = filter_channel_events(changed);

        // Check if our channel was updated
        let channel_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

        if channel_changes.contains(&channel_name.to_string()) {
            let (new_messages, new_offset): (Vec<Message>, u64) =
                read_records_from_offset(path, offset)?;

            for msg in &new_messages {
                print_message(msg);
            }

            offset = new_offset;
        }
    }
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
    fn test_history_basic() {
        let temp = setup();
        send::run(
            "general".to_string(),
            "Message 1".to_string(),
            None,
            Some("Historian"),
            temp.path(),
        )
        .unwrap();
        send::run(
            "general".to_string(),
            "Message 2".to_string(),
            None,
            Some("Historian"),
            temp.path(),
        )
        .unwrap();

        let options = HistoryOptions {
            channel: Some("general".to_string()),
            count: 50,
            follow: false,
            since: None,
            before: None,
            from: None,
        };

        run(options, temp.path()).unwrap();
    }

    #[test]
    fn test_history_empty_channel() {
        let temp = setup();

        let options = HistoryOptions {
            channel: Some("empty".to_string()),
            count: 50,
            follow: false,
            since: None,
            before: None,
            from: None,
        };

        run(options, temp.path()).unwrap();
    }

    #[test]
    fn test_history_filter_from() {
        let temp = setup();
        send::run(
            "general".to_string(),
            "From Historian".to_string(),
            None,
            Some("Historian"),
            temp.path(),
        )
        .unwrap();

        let options = HistoryOptions {
            channel: Some("general".to_string()),
            count: 50,
            follow: false,
            since: None,
            before: None,
            from: Some("Historian".to_string()),
        };

        run(options, temp.path()).unwrap();
    }

    #[test]
    fn test_parse_datetime() {
        assert!(parse_datetime("2026-01-23").is_ok());
        assert!(parse_datetime("2026-01-23T12:00:00Z").is_ok());
        assert!(parse_datetime("2026-01-23 12:00:00").is_ok());
        assert!(parse_datetime("invalid").is_err());
    }
}
