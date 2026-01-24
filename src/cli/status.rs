//! Project status overview command.

use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::claim::FileClaim;
use crate::core::identity::resolve_agent;
use crate::core::message::Message;
use crate::core::project::{agents_path, channels_dir, claims_path};
use crate::storage::agent_state::AgentStateManager;
use crate::storage::jsonl::{count_records, read_records};

/// Status output for JSON serialization.
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    /// Current agent identity (if any)
    pub agent: Option<String>,
    /// Number of registered agents
    pub agent_count: usize,
    /// Agents active in last 30 minutes
    pub active_agents: Vec<String>,
    /// Channel summaries
    pub channels: Vec<ChannelStatus>,
    /// My active claims
    pub my_claims: Vec<ClaimStatus>,
    /// Other agents' active claims
    pub other_claims: Vec<ClaimStatus>,
    /// Total unread messages (if agent identity available)
    pub unread_total: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ChannelStatus {
    pub name: String,
    pub message_count: usize,
    pub unread_count: Option<usize>,
    pub last_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ClaimStatus {
    pub agent: String,
    pub patterns: Vec<String>,
    pub expires_in_secs: i64,
}

/// Run status command.
pub fn run(json: bool, explicit_agent: Option<&str>, project_root: &Path) -> Result<()> {
    let output = collect_status(explicit_agent, project_root)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_status(&output);
    }

    Ok(())
}

fn collect_status(explicit_agent: Option<&str>, project_root: &Path) -> Result<StatusOutput> {
    let current_agent = resolve_agent(explicit_agent, project_root);
    let now = Utc::now();

    // Load agents
    let agents: Vec<Agent> = read_records(&agents_path(project_root)).unwrap_or_default();
    let last_seen = get_last_seen_times(project_root);

    // Find active agents (seen in last 30 min)
    let active_agents: Vec<String> = agents
        .iter()
        .filter(|a| {
            last_seen
                .get(&a.name)
                .is_some_and(|ts| now.signed_duration_since(*ts).num_minutes() < 30)
        })
        .map(|a| a.name.clone())
        .collect();

    // Load channels
    let channels = collect_channels(current_agent.as_deref(), project_root)?;

    // Calculate total unread
    let unread_total = if current_agent.is_some() {
        Some(channels.iter().filter_map(|c| c.unread_count).sum())
    } else {
        None
    };

    // Load claims
    let (my_claims, other_claims) = collect_claims(current_agent.as_deref(), project_root)?;

    Ok(StatusOutput {
        agent: current_agent,
        agent_count: agents.len(),
        active_agents,
        channels,
        my_claims,
        other_claims,
        unread_total,
    })
}

fn collect_channels(
    current_agent: Option<&str>,
    project_root: &Path,
) -> Result<Vec<ChannelStatus>> {
    let channels_path = channels_dir(project_root);
    if !channels_path.exists() {
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    let state_manager = current_agent.map(|a| AgentStateManager::new(project_root, a));

    for entry in std::fs::read_dir(&channels_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|e| e == "jsonl") {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            // Skip DM channels for cleaner output
            if name.contains("--") {
                continue;
            }

            let message_count = count_records(&path).unwrap_or(0);
            let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            // Get last message time
            let messages: Vec<Message> = read_records(&path).unwrap_or_default();
            let last_activity = messages.last().map(|m| m.ts);

            // Calculate unread if we have agent identity
            let unread_count = if let Some(ref manager) = state_manager {
                let cursor = manager.get_read_cursor(&name).unwrap_or_default();
                if cursor.offset < file_size {
                    // Count messages after offset (approximate)
                    Some(
                        messages
                            .iter()
                            .filter(|m| {
                                cursor
                                    .last_id
                                    .as_ref()
                                    .map_or(true, |last| m.id.to_string() > *last)
                            })
                            .count(),
                    )
                } else {
                    Some(0)
                }
            } else {
                None
            };

            results.push(ChannelStatus {
                name,
                message_count,
                unread_count,
                last_activity,
            });
        }
    }

    // Sort by last activity (most recent first)
    results.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));

    Ok(results)
}

fn collect_claims(
    current_agent: Option<&str>,
    project_root: &Path,
) -> Result<(Vec<ClaimStatus>, Vec<ClaimStatus>)> {
    let now = Utc::now();
    let all_claims: Vec<FileClaim> = read_records(&claims_path(project_root)).unwrap_or_default();

    // Build active claims (latest state per ID, not expired, still active)
    let mut active: HashMap<ulid::Ulid, FileClaim> = HashMap::new();
    for claim in all_claims {
        active.insert(claim.id, claim);
    }

    let mut my_claims = Vec::new();
    let mut other_claims = Vec::new();

    for claim in active.into_values() {
        if !claim.active || claim.expires_at < now {
            continue;
        }

        let status = ClaimStatus {
            agent: claim.agent.clone(),
            patterns: claim.patterns.clone(),
            expires_in_secs: (claim.expires_at - now).num_seconds(),
        };

        if current_agent.is_some_and(|a| a == claim.agent) {
            my_claims.push(status);
        } else {
            other_claims.push(status);
        }
    }

    Ok((my_claims, other_claims))
}

fn get_last_seen_times(project_root: &Path) -> HashMap<String, DateTime<Utc>> {
    let mut last_seen = HashMap::new();
    let channels = channels_dir(project_root);

    if !channels.exists() {
        return last_seen;
    }

    if let Ok(entries) = std::fs::read_dir(&channels) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Ok(messages) = read_records::<Message>(&path) {
                    for msg in messages {
                        let entry = last_seen.entry(msg.agent.clone()).or_insert(msg.ts);
                        if msg.ts > *entry {
                            *entry = msg.ts;
                        }
                    }
                }
            }
        }
    }

    last_seen
}

fn print_status(status: &StatusOutput) {
    // Header
    println!("{}", "BotBus Status".bold());
    println!();

    // Agent identity
    if let Some(agent) = &status.agent {
        println!("  {} {}", "You:".dimmed(), agent.cyan().bold());
    } else {
        println!("  {} {}", "You:".dimmed(), "(not identified)".yellow());
    }

    // Agents
    println!(
        "  {} {} registered, {} active",
        "Agents:".dimmed(),
        status.agent_count,
        status.active_agents.len()
    );
    if !status.active_agents.is_empty() {
        println!("          {}", status.active_agents.join(", ").green());
    }

    // Unread
    if let Some(unread) = status.unread_total {
        if unread > 0 {
            println!(
                "  {} {} unread",
                "Inbox:".dimmed(),
                unread.to_string().yellow().bold()
            );
        } else {
            println!("  {} all caught up", "Inbox:".dimmed());
        }
    }

    // Channels
    if !status.channels.is_empty() {
        println!();
        println!("{}", "Channels:".bold());
        for ch in &status.channels {
            let unread_str = ch
                .unread_count
                .map(|n| {
                    if n > 0 {
                        format!(" ({} new)", n).yellow().to_string()
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();

            let activity = ch
                .last_activity
                .map(|ts| format_time_ago(ts))
                .unwrap_or_else(|| "no activity".to_string());

            println!(
                "  #{:<15} {:>4} msgs, last: {}{}",
                ch.name.cyan(),
                ch.message_count,
                activity,
                unread_str
            );
        }
    }

    // Claims
    if !status.my_claims.is_empty() || !status.other_claims.is_empty() {
        println!();
        println!("{}", "Claims:".bold());

        for claim in &status.my_claims {
            let expires = format_duration_short(claim.expires_in_secs as u64);
            println!(
                "  {} {} (expires in {})",
                "You:".green(),
                claim.patterns.join(", ").cyan(),
                expires
            );
        }

        for claim in &status.other_claims {
            let expires = format_duration_short(claim.expires_in_secs as u64);
            println!(
                "  {}: {} (expires in {})",
                claim.agent.yellow(),
                claim.patterns.join(", ").cyan(),
                expires
            );
        }
    }

    println!();
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

fn format_duration_short(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{init, register, send};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_status_empty_project() {
        let temp = setup();
        run(false, None, temp.path()).unwrap();
    }

    #[test]
    fn test_status_with_agent() {
        let temp = setup();
        register::run(Some("TestAgent".to_string()), None, temp.path()).unwrap();
        run(false, Some("TestAgent"), temp.path()).unwrap();
    }

    #[test]
    fn test_status_json() {
        let temp = setup();
        register::run(Some("TestAgent".to_string()), None, temp.path()).unwrap();
        send::run(
            "general".to_string(),
            "Hello".to_string(),
            None,
            Some("TestAgent"),
            temp.path(),
        )
        .unwrap();

        run(true, Some("TestAgent"), temp.path()).unwrap();
    }

    #[test]
    fn test_status_output_structure() {
        let temp = setup();
        register::run(Some("Agent1".to_string()), None, temp.path()).unwrap();
        register::run(Some("Agent2".to_string()), None, temp.path()).unwrap();

        let output = collect_status(Some("Agent1"), temp.path()).unwrap();

        assert_eq!(output.agent, Some("Agent1".to_string()));
        assert_eq!(output.agent_count, 2);
        assert!(!output.channels.is_empty()); // general has registration messages
    }
}
