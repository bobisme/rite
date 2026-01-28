//! List all channels.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;

use crate::core::channel::{dm_agents, is_dm_channel};
use crate::core::identity::resolve_agent;
use crate::core::message::Message;
use crate::core::project::channels_dir;
use crate::storage::jsonl::{count_records, read_last_n, read_records};

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
/// If `mine_only` is true, only show channels where the agent has participated.
pub fn run(json: bool, mine_only: bool, agent: Option<&str>) -> Result<()> {
    let current_agent = resolve_agent(agent);
    let channels_path = channels_dir();

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

        // If --mine, filter to channels where agent participated
        if mine_only
            && let Some(ref agent) = current_agent
                && !has_participated(&path, agent, &channel_name) {
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
            .map(format_time_ago)
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

/// Check if an agent has participated in a channel.
/// Participation means: sent a message OR was @mentioned OR is part of a DM.
fn has_participated(path: &std::path::Path, agent: &str, channel_name: &str) -> bool {
    // For DM channels, check if the agent is one of the participants
    if is_dm_channel(channel_name)
        && let Some((a, b)) = dm_agents(channel_name)
            && (a == agent || b == agent) {
                return true;
            }

    // Check if agent sent any messages or was @mentioned
    let messages: Vec<Message> = read_records(path).unwrap_or_default();
    for msg in messages {
        // Agent sent a message
        if msg.agent == agent {
            return true;
        }
        // Agent was @mentioned
        if msg.body.contains(&format!("@{}", agent)) {
            return true;
        }
    }

    false
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
    use crate::cli::send;
    use crate::core::project::{DATA_DIR_ENV_VAR, ensure_data_dir};
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    struct TestEnv {
        _dir: TempDir,
    }

    impl TestEnv {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            unsafe {
                env::set_var(DATA_DIR_ENV_VAR, dir.path());
            }
            ensure_data_dir().unwrap();
            Self { _dir: dir }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                env::remove_var(DATA_DIR_ENV_VAR);
            }
        }
    }

    #[test]
    #[serial]
    fn test_list_channels() {
        let _env = TestEnv::new();
        send::run_simple(
            "test-backend".to_string(),
            "test".to_string(),
            Some("test-agent"),
        )
        .unwrap();

        // Show all channels (default)
        run(false, false, None).unwrap();
    }

    #[test]
    #[serial]
    fn test_list_channels_json() {
        let _env = TestEnv::new();

        run(true, false, None).unwrap();
    }

    #[test]
    #[serial]
    fn test_list_with_mine_filter() {
        let _env = TestEnv::new();
        send::run_simple(
            "@test-other".to_string(),
            "dm".to_string(),
            Some("test-agent"),
        )
        .unwrap();

        // With --mine filter, should only show channels where agent participated
        run(false, true, Some("test-agent")).unwrap();
    }
}
