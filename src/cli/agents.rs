//! List agents derived from message history.

use anyhow::Result;
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;

use crate::core::message::Message;
use crate::core::project::channels_dir;
use crate::storage::jsonl::read_records;

#[derive(Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub last_seen: DateTime<Utc>,
    pub message_count: usize,
    pub active: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentsOutput {
    pub agents: Vec<AgentInfo>,
}

/// List agents derived from message history.
pub fn run(json: bool, _active_only: bool) -> Result<()> {
    let agent_stats = get_agent_stats();
    let now = Utc::now();

    let mut agent_infos: Vec<AgentInfo> = agent_stats
        .into_iter()
        .map(|(name, (last_seen, count))| {
            let active = now.signed_duration_since(last_seen).num_minutes() < 30;
            AgentInfo {
                name,
                last_seen,
                message_count: count,
                active,
            }
        })
        .collect();

    // Sort by last seen (most recent first)
    agent_infos.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));

    if json {
        let output = AgentsOutput {
            agents: agent_infos,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if agent_infos.is_empty() {
        println!("No agents found in message history.");
        return Ok(());
    }

    println!("{}", "Agents (from message history):".bold());

    for info in &agent_infos {
        let last_seen_str = format_time_ago(info.last_seen);

        let indicator = if info.active {
            "●".green()
        } else {
            "○".dimmed()
        };

        println!(
            "  {} {:<24} last seen {}, {} messages",
            indicator,
            info.name.cyan(),
            last_seen_str,
            info.message_count
        );
    }

    Ok(())
}

/// Scan all channels and collect agent statistics.
fn get_agent_stats() -> HashMap<String, (DateTime<Utc>, usize)> {
    let mut stats: HashMap<String, (DateTime<Utc>, usize)> = HashMap::new();

    let channels = channels_dir();
    if !channels.exists() {
        return stats;
    }

    if let Ok(entries) = std::fs::read_dir(&channels) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Ok(messages) = read_records::<Message>(&path)
            {
                for msg in messages {
                    let entry = stats.entry(msg.agent.clone()).or_insert((msg.ts, 0));
                    if msg.ts > entry.0 {
                        entry.0 = msg.ts;
                    }
                    entry.1 += 1;
                }
            }
        }
    }

    stats
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
    fn test_list_agents() {
        let _env = TestEnv::new();

        // Send a message to create an agent in history
        send::run_simple(
            "test-agents-channel".to_string(),
            "test message".to_string(),
            Some("test-agent-1"),
        )
        .unwrap();

        run(false, false).unwrap();
    }

    #[test]
    #[serial]
    fn test_list_agents_json() {
        let _env = TestEnv::new();

        run(true, false).unwrap();
    }
}
