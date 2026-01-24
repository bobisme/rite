use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::message::Message;
use crate::core::project::{agents_path, channels_dir};
use crate::storage::jsonl::read_records;

#[derive(Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub last_seen: Option<DateTime<Utc>>,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentsOutput {
    pub agents: Vec<AgentInfo>,
}

/// List registered agents.
pub fn run(json: bool, _active_only: bool, project_root: &Path) -> Result<()> {
    let agents: Vec<Agent> =
        read_records(&agents_path(project_root)).with_context(|| "Failed to read agents")?;

    let last_seen = get_last_seen_times(project_root);
    let now = Utc::now();

    let agent_infos: Vec<AgentInfo> = agents
        .iter()
        .map(|a| {
            let seen = last_seen.get(&a.name).copied();
            let active = seen.is_some_and(|ts| now.signed_duration_since(ts).num_minutes() < 30);
            AgentInfo {
                name: a.name.clone(),
                description: a.description.clone(),
                registered_at: a.ts,
                last_seen: seen,
                active,
            }
        })
        .collect();

    if json {
        let output = AgentsOutput {
            agents: agent_infos,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if agent_infos.is_empty() {
        println!("No agents registered yet.");
        return Ok(());
    }

    println!("{}", "Agents:".bold());

    for info in &agent_infos {
        let registered_ago = format_time_ago(info.registered_at);
        let last_seen_str = info
            .last_seen
            .map(|ts| format_time_ago(ts))
            .unwrap_or_else(|| "never".to_string());

        let indicator = if info.active {
            "●".green()
        } else {
            "○".dimmed()
        };

        println!(
            "  {} {:<20} Registered {}, last seen {}",
            indicator,
            info.name.cyan(),
            registered_ago,
            last_seen_str
        );

        if let Some(desc) = &info.description {
            println!("                         {}", desc.dimmed());
        }
    }

    Ok(())
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
    use crate::cli::{init, register};
    use tempfile::TempDir;

    #[test]
    fn test_list_agents() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        register::run(
            Some("Agent1".to_string()),
            Some("First".to_string()),
            temp.path(),
        )
        .unwrap();

        run(false, false, temp.path()).unwrap();
    }

    #[test]
    fn test_list_agents_json() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        register::run(Some("Agent1".to_string()), None, temp.path()).unwrap();

        run(true, false, temp.path()).unwrap();
    }

    #[test]
    fn test_no_agents() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();

        run(false, false, temp.path()).unwrap();
    }
}
