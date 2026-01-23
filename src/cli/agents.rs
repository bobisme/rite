use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::message::Message;
use crate::core::project::{agents_path, channels_dir};
use crate::storage::jsonl::read_records;

/// List registered agents.
pub fn run(_active_only: bool, project_root: &Path) -> Result<()> {
    let agents: Vec<Agent> =
        read_records(&agents_path(project_root)).with_context(|| "Failed to read agents")?;

    if agents.is_empty() {
        println!("No agents registered yet.");
        return Ok(());
    }

    // Build a map of agent -> last message time
    let last_seen = get_last_seen_times(project_root);

    println!("{}", "Agents:".bold());

    for agent in &agents {
        let registered_ago = format_time_ago(agent.ts);
        let last_seen_str = last_seen
            .get(&agent.name)
            .map(|ts| format_time_ago(*ts))
            .unwrap_or_else(|| "never".to_string());

        // Activity indicator
        let indicator = if last_seen
            .get(&agent.name)
            .is_some_and(|ts| chrono::Utc::now().signed_duration_since(*ts).num_minutes() < 30)
        {
            "●".green()
        } else {
            "○".dimmed()
        };

        println!(
            "  {} {:<20} Registered {}, last seen {}",
            indicator,
            agent.name.cyan(),
            registered_ago,
            last_seen_str
        );

        if let Some(desc) = &agent.description {
            println!("                         {}", desc.dimmed());
        }
    }

    Ok(())
}

fn get_last_seen_times(
    project_root: &Path,
) -> std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> {
    let mut last_seen = std::collections::HashMap::new();

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

        // Should not error
        run(false, temp.path()).unwrap();
    }

    #[test]
    fn test_no_agents() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();

        // Should not error
        run(false, temp.path()).unwrap();
    }
}
