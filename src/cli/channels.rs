use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::channel::is_dm_channel;
use crate::core::message::Message;
use crate::core::project::channels_dir;
use crate::storage::jsonl::{count_records, read_last_n};

/// List all channels.
pub fn run(include_dms: bool, project_root: &Path) -> Result<()> {
    let channels = channels_dir(project_root);

    if !channels.exists() {
        println!("No channels yet.");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&channels)
        .with_context(|| "Failed to read channels directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    if entries.is_empty() {
        println!("No channels yet.");
        return Ok(());
    }

    // Sort by modification time (most recent first)
    entries.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).ok();
        let b_time = b.metadata().and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });

    println!("{}", "Channels:".bold());

    for entry in entries {
        let path = entry.path();
        let channel_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        // Skip DMs unless --all
        if is_dm_channel(channel_name) && !include_dms {
            continue;
        }

        let count = count_records(&path).unwrap_or(0);
        let last_msg: Option<Message> = read_last_n(&path, 1)
            .ok()
            .and_then(|v: Vec<Message>| v.into_iter().next());

        let time_ago = last_msg
            .map(|m| format_time_ago(m.ts))
            .unwrap_or_else(|| "never".to_string());

        let prefix = if is_dm_channel(channel_name) { "" } else { "#" };

        println!(
            "  {}{:<20} {:>4} messages, last: {}",
            prefix,
            channel_name.cyan(),
            count,
            time_ago
        );
    }

    Ok(())
}

fn format_time_ago(ts: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(ts);

    if duration.num_seconds() < 60 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}d ago", duration.num_days())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{init, send};
    use tempfile::TempDir;

    #[test]
    fn test_list_channels() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        send::run(
            "backend".to_string(),
            "test".to_string(),
            None,
            Some("Agent"),
            temp.path(),
        )
        .unwrap();

        // Should not error
        run(false, temp.path()).unwrap();
    }

    #[test]
    fn test_list_with_dms() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        send::run(
            "@Other".to_string(),
            "dm".to_string(),
            None,
            Some("Agent"),
            temp.path(),
        )
        .unwrap();

        // Should not error
        run(true, temp.path()).unwrap();
    }
}
