//! List all channels.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;

use super::OutputFormat;
use super::format::to_toon;
use crate::core::channel::{dm_agents, is_dm_channel};
use crate::core::identity::resolve_agent;
use crate::core::message::{Message, read_last_n_messages, read_messages};
use crate::core::project::{channel_path, channels_dir, index_path, state_path};
use crate::storage::jsonl::count_records;
use crate::storage::state::ProjectState;

#[derive(Debug, Serialize)]
pub struct ChannelInfo {
    pub name: String,
    pub is_dm: bool,
    pub message_count: usize,
    pub last_activity: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ChannelsOutput {
    pub channels: Vec<ChannelInfo>,
}

/// List all channels.
/// If `mine_only` is true, only show channels where the agent has participated.
/// If `show_all` is true, include closed channels.
pub fn list(
    format: OutputFormat,
    mine_only: bool,
    show_all: bool,
    agent: Option<&str>,
) -> Result<()> {
    let current_agent = resolve_agent(agent);
    let channels_path = channels_dir();

    // Load closed channels list from state
    let state_file = ProjectState::new(state_path());
    let state = state_file.load()?;
    let closed_channels = &state.closed_channels;

    if !channels_path.exists() {
        match format {
            OutputFormat::Json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ChannelsOutput { channels: vec![] })?
                );
            }
            OutputFormat::Toon => {
                println!("channels: []");
            }
            OutputFormat::Text => {
                println!("No channels yet.");
            }
        }
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&channels_path)
        .with_context(|| "Failed to read channels directory")?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();

    if entries.is_empty() {
        match format {
            OutputFormat::Json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ChannelsOutput { channels: vec![] })?
                );
            }
            OutputFormat::Toon => {
                println!("channels: []");
            }
            OutputFormat::Text => {
                println!("No channels yet.");
            }
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
        let is_closed = closed_channels.contains(&channel_name);

        // Filter out closed channels unless --all is specified
        if is_closed && !show_all {
            continue;
        }

        // If --mine, filter to channels where agent participated
        if mine_only
            && let Some(ref agent) = current_agent
            && !has_participated(&path, agent, &channel_name)
        {
            continue;
        }

        let message_count = count_records(&path).unwrap_or(0);
        let last_msg: Option<Message> = read_last_n_messages(&path, 1)
            .ok()
            .and_then(|v: Vec<Message>| v.into_iter().next());

        channel_infos.push(ChannelInfo {
            name: channel_name,
            is_dm,
            message_count,
            last_activity: last_msg.map(|m| m.ts),
            closed: if is_closed { Some(true) } else { None },
        });
    }

    let output = ChannelsOutput {
        channels: channel_infos,
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Toon => {
            println!("{}", to_toon(&output));
        }
        OutputFormat::Text => {
            println!("{}", "Channels:".bold());

            for info in &output.channels {
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
        }
    }

    Ok(())
}

/// Check if an agent has participated in a channel.
/// Participation means: sent a message OR was @mentioned OR is part of a DM.
fn has_participated(path: &std::path::Path, agent: &str, channel_name: &str) -> bool {
    // For DM channels, check if the agent is one of the participants
    if is_dm_channel(channel_name)
        && let Some((a, b)) = dm_agents(channel_name)
        && (a == agent || b == agent)
    {
        return true;
    }

    // Check if agent sent any messages or was @mentioned
    let messages: Vec<Message> = read_messages(path).unwrap_or_default();
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

/// Close a channel (hide from listings, preserves history).
pub fn close(channel: &str) -> Result<()> {
    let state_file = ProjectState::new(state_path());
    let mut state = state_file.load()?;

    // Check if already closed
    if state.closed_channels.contains(&channel.to_string()) {
        anyhow::bail!("Channel '{}' is already closed", channel);
    }

    // Add to closed list
    state.closed_channels.push(channel.to_string());
    state_file.save(&state)?;

    println!("Closed channel '{}'", channel);
    Ok(())
}

/// Reopen a closed channel.
pub fn reopen(channel: &str) -> Result<()> {
    let state_file = ProjectState::new(state_path());
    let mut state = state_file.load()?;

    // Check if actually closed
    if !state.closed_channels.contains(&channel.to_string()) {
        anyhow::bail!("Channel '{}' is not closed", channel);
    }

    // Remove from closed list
    state.closed_channels.retain(|c| c != channel);
    state_file.save(&state)?;

    println!("Reopened channel '{}'", channel);
    Ok(())
}

/// Delete a channel permanently (admin only).
///
/// IMPORTANT: This command is for administrators only. It permanently deletes
/// all channel data including messages, read cursors, and index entries.
/// Agents should not use this command as it requires interactive confirmation.
pub fn delete(channel: &str) -> Result<()> {
    use std::io::{self, Write};

    // Print admin warning
    eprintln!("\n{}", "⚠️  WARNING".bold().red());
    eprintln!("This command is for administrators only.");
    eprintln!("It permanently deletes all channel data.\n");

    // Check if channel exists
    let channel_file = channel_path(channel);
    if !channel_file.exists() {
        anyhow::bail!("Channel '{}' does not exist", channel);
    }

    // Interactive confirmation prompt (will block if no stdin available)
    eprint!("Type {} to confirm: ", format!("delete {}", channel).bold());
    io::stderr().flush()?;

    let mut confirmation = String::new();
    io::stdin()
        .read_line(&mut confirmation)
        .context("Failed to read confirmation")?;

    let expected = format!("delete {}", channel);
    if confirmation.trim() != expected {
        anyhow::bail!("Confirmation did not match. Deletion aborted.");
    }

    // Perform deletion in order: state → index → file

    // 1. Remove from state (read cursors, closed channels list)
    let state_file = ProjectState::new(state_path());
    let mut state = state_file.load()?;
    state.channel_offsets.remove(channel);
    state.last_seen.remove(channel);
    state.index_offsets.remove(channel);
    state.closed_channels.retain(|c| c != channel);
    state_file.save(&state)?;
    eprintln!("✓ Removed from state");

    // 2. Remove from FTS index
    let index = index_path();
    if index.exists() {
        use rusqlite::Connection;
        let conn = Connection::open(&index)?;
        conn.execute("DELETE FROM messages WHERE channel = ?1", [channel])?;
        eprintln!("✓ Removed from search index");
    }

    // 3. Delete the channel file
    std::fs::remove_file(&channel_file)
        .with_context(|| format!("Failed to delete channel file: {}", channel_file.display()))?;
    eprintln!("✓ Deleted channel file");

    eprintln!("\n{}", "Channel deleted successfully.".green());
    Ok(())
}

/// Rename a channel (admin only).
///
/// IMPORTANT: This command is for administrators only. It renames a channel
/// and updates all references in state and index.
/// Agents should not use this command as it requires interactive confirmation.
pub fn rename(old_name: &str, new_name: &str) -> Result<()> {
    use std::io::{self, Write};

    // Print admin warning
    eprintln!("\n{}", "⚠️  WARNING".bold().red());
    eprintln!("This command is for administrators only.");
    eprintln!("It renames a channel and updates all references.\n");

    // Check if old channel exists
    let old_file = channel_path(old_name);
    if !old_file.exists() {
        anyhow::bail!("Channel '{}' does not exist", old_name);
    }

    // Check if new channel already exists
    let new_file = channel_path(new_name);
    if new_file.exists() {
        anyhow::bail!("Channel '{}' already exists", new_name);
    }

    // Interactive confirmation prompt (will block if no stdin available)
    eprint!(
        "Type {} to confirm: ",
        format!("rename {} {}", old_name, new_name).bold()
    );
    io::stderr().flush()?;

    let mut confirmation = String::new();
    io::stdin()
        .read_line(&mut confirmation)
        .context("Failed to read confirmation")?;

    let expected = format!("rename {} {}", old_name, new_name);
    if confirmation.trim() != expected {
        anyhow::bail!("Confirmation did not match. Rename aborted.");
    }

    // Perform rename in order: state → index → file

    // 1. Update state (rename keys in maps and closed_channels list)
    let state_file = ProjectState::new(state_path());
    let mut state = state_file.load()?;

    // Rename in channel_offsets
    if let Some(offset) = state.channel_offsets.remove(old_name) {
        state.channel_offsets.insert(new_name.to_string(), offset);
    }

    // Rename in last_seen
    if let Some(last_seen) = state.last_seen.remove(old_name) {
        state.last_seen.insert(new_name.to_string(), last_seen);
    }

    // Rename in index_offsets
    if let Some(index_offset) = state.index_offsets.remove(old_name) {
        state
            .index_offsets
            .insert(new_name.to_string(), index_offset);
    }

    // Rename in closed_channels
    if let Some(pos) = state.closed_channels.iter().position(|c| c == old_name) {
        state.closed_channels[pos] = new_name.to_string();
    }

    state_file.save(&state)?;
    eprintln!("✓ Updated state");

    // 2. Update agent states
    use crate::core::project::data_dir;
    use crate::storage::agent_state::rename_channel_in_agent_states;
    let updated_agents = rename_channel_in_agent_states(&data_dir(), old_name, new_name)
        .with_context(|| {
            format!(
                "Failed to update agent states for channel rename: {} → {}",
                old_name, new_name
            )
        })?;
    if updated_agents > 0 {
        eprintln!("✓ Updated {} agent state(s)", updated_agents);
    }

    // 3. Update FTS index
    let index = index_path();
    if index.exists() {
        use rusqlite::Connection;
        let conn = Connection::open(&index)?;
        conn.execute(
            "UPDATE messages SET channel = ?1 WHERE channel = ?2",
            [new_name, old_name],
        )?;
        // Update sync_state table (primary key can be updated in SQLite)
        conn.execute(
            "UPDATE sync_state SET channel = ?1 WHERE channel = ?2",
            [new_name, old_name],
        )?;
        eprintln!("✓ Updated search index");
    }

    // 4. Update hooks that reference this channel
    let updated_hooks = super::hooks::rename_channel_in_hooks(old_name, new_name)?;
    if updated_hooks > 0 {
        eprintln!("✓ Updated {} hook(s)", updated_hooks);
    }

    // 5. Rename the channel file
    std::fs::rename(&old_file, &new_file).with_context(|| {
        format!(
            "Failed to rename channel file from {} to {}",
            old_file.display(),
            new_file.display()
        )
    })?;
    eprintln!("✓ Renamed channel file");

    // 6. Update Telegram config if channel has topic mapping
    match crate::telegram::config::rename_channel_in_telegram_config(old_name, new_name) {
        Ok(true) => eprintln!("✓ Updated Telegram mapping"),
        Ok(false) => {} // No mapping or no config - skip silently
        Err(e) => eprintln!("⚠ Warning: Failed to update Telegram config: {}", e),
    }

    eprintln!(
        "\n{}",
        format!("Channel renamed: {} → {}", old_name, new_name).green()
    );
    Ok(())
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
        list(OutputFormat::Text, false, false, None).unwrap();
    }

    #[test]
    #[serial]
    fn test_list_channels_json() {
        let _env = TestEnv::new();

        list(OutputFormat::Json, false, false, None).unwrap();
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
        list(OutputFormat::Text, true, false, Some("test-agent")).unwrap();
    }

    #[test]
    #[serial]
    fn test_rename_updates_agent_states() {
        use crate::storage::agent_state::AgentStateManager;

        let _env = TestEnv::new();

        // Send a message to create a channel
        send::run_simple(
            "old-channel".to_string(),
            "test message".to_string(),
            Some("test-agent"),
        )
        .unwrap();

        // Set up agent state with references to old channel
        let data_dir = crate::core::project::data_dir();
        let manager = AgentStateManager::new(&data_dir, "test-agent");

        // Add channel to agent state
        manager.subscribe("old-channel").unwrap();
        manager.set_read_offset("old-channel", 100).unwrap();
        manager
            .set_last_read_id("old-channel", "test-id-123")
            .unwrap();

        // Verify initial state
        let state = manager.load().unwrap();
        assert!(
            state
                .subscribed_channels
                .contains(&"old-channel".to_string())
        );
        assert_eq!(state.read_offsets.get("old-channel"), Some(&100));
        assert_eq!(
            state.last_read_ids.get("old-channel"),
            Some(&"test-id-123".to_string())
        );

        // Mock interactive input by testing the helper function directly
        use crate::storage::agent_state::rename_channel_in_agent_states;
        let updated = rename_channel_in_agent_states(&data_dir, "old-channel", "new-channel")
            .expect("Failed to rename channel in agent states");

        // Should have updated 1 agent state
        assert_eq!(updated, 1);

        // Verify agent state was updated
        let state = manager.load().unwrap();
        assert!(
            !state
                .subscribed_channels
                .contains(&"old-channel".to_string())
        );
        assert!(
            state
                .subscribed_channels
                .contains(&"new-channel".to_string())
        );
        assert_eq!(state.read_offsets.get("new-channel"), Some(&100));
        assert_eq!(
            state.last_read_ids.get("new-channel"),
            Some(&"test-id-123".to_string())
        );
        assert_eq!(state.read_offsets.get("old-channel"), None);
        assert_eq!(state.last_read_ids.get("old-channel"), None);
    }
}
