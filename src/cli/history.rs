use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use colored::Colorize;
use std::path::Path;

use crate::core::message::Message;
use crate::core::project::channel_path;
use crate::storage::jsonl::{read_last_n, read_records, read_records_from_offset};

#[derive(Clone)]
pub struct HistoryOptions {
    pub channel: Option<String>,
    pub count: usize,
    pub follow: bool,
    pub since: Option<String>,
    pub before: Option<String>,
    pub from: Option<String>,
    /// Read messages after this byte offset (for incremental reading)
    pub after_offset: Option<u64>,
    /// Read messages after this message ID (ULID)
    pub after_id: Option<String>,
    /// Show the offset info for next read
    pub show_offset: bool,
}

/// Output from history command, useful for programmatic access.
#[derive(Debug)]
pub struct HistoryOutput {
    pub messages: Vec<Message>,
    /// Byte offset for next read (end of file after this read)
    pub next_offset: u64,
    /// ID of the last message returned (if any)
    pub last_id: Option<String>,
}

/// View message history.
pub fn run(options: HistoryOptions, project_root: &Path) -> Result<()> {
    let output = run_with_output(options.clone(), project_root)?;

    let channel = options.channel.unwrap_or_else(|| "general".to_string());

    if output.messages.is_empty() {
        if options.after_offset.is_some() || options.after_id.is_some() {
            println!("No new messages.");
        } else {
            println!("No messages match your criteria.");
        }

        // Still show offset info if requested
        if options.show_offset {
            println!("{}: {}", "next_offset".dimmed(), output.next_offset);
        }
        return Ok(());
    }

    // Print header
    println!("{}", format!("#{}", channel).cyan().bold());

    // Print messages
    for msg in &output.messages {
        print_message(msg);
    }

    // Show offset info for next read
    if options.show_offset {
        println!();
        println!("{}: {}", "next_offset".dimmed(), output.next_offset);
        if let Some(last_id) = &output.last_id {
            println!("{}: {}", "last_id".dimmed(), last_id);
        }
    }

    // Follow mode
    if options.follow {
        let path = channel_path(project_root, &channel);
        follow_channel(&path, project_root)?;
    }

    Ok(())
}

/// Run history and return structured output (for programmatic use).
pub fn run_with_output(options: HistoryOptions, project_root: &Path) -> Result<HistoryOutput> {
    let channel = options
        .channel
        .clone()
        .unwrap_or_else(|| "general".to_string());
    let path = channel_path(project_root, &channel);

    if !path.exists() {
        return Ok(HistoryOutput {
            messages: Vec::new(),
            next_offset: 0,
            last_id: None,
        });
    }

    // Get file size for next_offset calculation
    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    // Read messages based on options
    let (messages, next_offset) = if let Some(offset) = options.after_offset {
        // Read from specific offset
        let (msgs, new_offset): (Vec<Message>, u64) = read_records_from_offset(&path, offset)
            .with_context(|| format!("Failed to read channel #{} from offset", channel))?;
        (msgs, new_offset)
    } else if let Some(after_id) = &options.after_id {
        // Read all and filter to messages after the given ID
        let all: Vec<Message> =
            read_records(&path).with_context(|| format!("Failed to read channel #{}", channel))?;

        // Find the position of the after_id message
        let start_idx = all
            .iter()
            .position(|m| m.id.to_string() == *after_id)
            .map(|i| i + 1) // Start after the found message
            .unwrap_or(0); // If not found, return all messages

        let msgs: Vec<Message> = all.into_iter().skip(start_idx).collect();
        (msgs, file_size)
    } else if options.since.is_some() || options.before.is_some() || options.from.is_some() {
        // Need to filter, read all and filter
        let all: Vec<Message> =
            read_records(&path).with_context(|| format!("Failed to read channel #{}", channel))?;
        (filter_messages(all, &options), file_size)
    } else {
        // Just get last N
        let msgs = read_last_n(&path, options.count)
            .with_context(|| format!("Failed to read channel #{}", channel))?;
        (msgs, file_size)
    };

    // Apply count limit if we used after_offset or after_id
    let messages = if (options.after_offset.is_some() || options.after_id.is_some())
        && messages.len() > options.count
    {
        messages.into_iter().take(options.count).collect()
    } else {
        messages
    };

    let last_id = messages.last().map(|m| m.id.to_string());

    Ok(HistoryOutput {
        messages,
        next_offset,
        last_id,
    })
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
            after_offset: None,
            after_id: None,
            show_offset: false,
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
            after_offset: None,
            after_id: None,
            show_offset: false,
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
            after_offset: None,
            after_id: None,
            show_offset: false,
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
