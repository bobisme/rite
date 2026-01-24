use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use std::path::Path;

use crate::core::channel::is_dm_channel;
use crate::core::message::Message;
use crate::core::project::channels_dir;
use crate::storage::jsonl::{count_records, read_last_n};

#[derive(Debug, Serialize)]
pub struct ChannelInfo {
    pub name: String,
    pub is_dm: bool,
    pub message_count: usize,
    pub last_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ChannelsOutput {
    pub channels: Vec<ChannelInfo>,
}

/// List all channels.
pub fn run(json: bool, include_dms: bool, project_root: &Path) -> Result<()> {
    let channels_path = channels_dir(project_root);

    if !channels_path.exists() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&ChannelsOutput { channels: vec![] })?
            );
        } else {
            println!("No channels yet.");
        }
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&channels_path)
        .with_context(|| "Failed to read channels directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    if entries.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&ChannelsOutput { channels: vec![] })?
            );
        } else {
            println!("No channels yet.");
        }
        return Ok(());
    }

    // Sort by modification time (most recent first)
    entries.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).ok();
        let b_time = b.metadata().and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });

    let mut channel_infos: Vec<ChannelInfo> = Vec::new();

    for entry in entries {
        let path = entry.path();
        let channel_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let is_dm = is_dm_channel(&channel_name);

        // Skip DMs unless --all
        if is_dm && !include_dms {
            continue;
        }

        let message_count = count_records(&path).unwrap_or(0);
        let last_msg: Option<Message> = read_last_n(&path, 1)
            .ok()
            .and_then(|v: Vec<Message>| v.into_iter().next());

        channel_infos.push(ChannelInfo {
            name: channel_name,
            is_dm,
            message_count,
            last_activity: last_msg.map(|m| m.ts),
        });
    }

    if json {
        let output = ChannelsOutput {
            channels: channel_infos,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("{}", "Channels:".bold());

    for info in &channel_infos {
        let time_ago = info
            .last_activity
            .map(|ts| format_time_ago(ts))
            .unwrap_or_else(|| "never".to_string());

        let prefix = if info.is_dm { "" } else { "#" };

        println!(
            "  {}{:<20} {:>4} messages, last: {}",
            prefix,
            info.name.cyan(),
            info.message_count,
            time_ago
        );
    }

    Ok(())
}

fn format_time_ago(ts: DateTime<Utc>) -> String {
    let now = Utc::now();
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
        send::run_simple(
            "backend".to_string(),
            "test".to_string(),
            Some("Agent"),
            temp.path(),
        )
        .unwrap();

        run(false, false, temp.path()).unwrap();
    }

    #[test]
    fn test_list_channels_json() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        send::run_simple(
            "backend".to_string(),
            "test".to_string(),
            Some("Agent"),
            temp.path(),
        )
        .unwrap();

        run(true, false, temp.path()).unwrap();
    }

    #[test]
    fn test_list_with_dms() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        send::run_simple(
            "@Other".to_string(),
            "dm".to_string(),
            Some("Agent"),
            temp.path(),
        )
        .unwrap();

        run(false, true, temp.path()).unwrap();
    }
}
