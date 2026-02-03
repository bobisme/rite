use anyhow::Result;
use chrono::Utc;
use colored::Colorize;

use super::OutputFormat;
use super::format::to_toon_list;
use crate::core::identity::require_agent;
use crate::core::project::statuses_path;
use crate::core::status::AgentStatusEntry;
use crate::storage::jsonl::{append_record, read_records};

/// Parse a TTL string like "1h", "30m", "8h", "3600" into seconds.
fn parse_ttl(ttl: &str) -> Result<u64> {
    let ttl = ttl.trim();
    if let Some(hours) = ttl.strip_suffix('h') {
        Ok(hours.parse::<u64>()? * 3600)
    } else if let Some(mins) = ttl.strip_suffix('m') {
        Ok(mins.parse::<u64>()? * 60)
    } else if let Some(secs) = ttl.strip_suffix('s') {
        Ok(secs.parse::<u64>()?)
    } else {
        Ok(ttl.parse::<u64>()?)
    }
}

pub fn set(message: &str, ttl: &str, agent: Option<&str>, _format: OutputFormat) -> Result<()> {
    let agent_name = require_agent(agent)?;

    // Truncate message to 32 chars
    let message = if message.len() > 32 {
        &message[..32]
    } else {
        message
    };

    let ttl_secs = parse_ttl(ttl)?;
    let entry = AgentStatusEntry::new(&agent_name, message, ttl_secs);
    append_record(&statuses_path(), &entry)?;

    println!(
        "{} Status set for {}",
        "Success:".green(),
        format_duration(ttl_secs)
    );
    println!("  {} {}", agent_name.cyan(), message.dimmed());

    Ok(())
}

pub fn clear(agent: Option<&str>, _format: OutputFormat) -> Result<()> {
    let agent_name = require_agent(agent)?;

    let entry = AgentStatusEntry::clear(&agent_name);
    append_record(&statuses_path(), &entry)?;

    println!("{} Status cleared", "Success:".green());

    Ok(())
}

pub fn list(format: OutputFormat, _agent: Option<&str>) -> Result<()> {
    let all_entries: Vec<AgentStatusEntry> = read_records(&statuses_path()).unwrap_or_default();

    // Build latest status per agent
    let mut latest: std::collections::HashMap<String, AgentStatusEntry> =
        std::collections::HashMap::new();
    for entry in all_entries {
        latest.insert(entry.agent.clone(), entry);
    }

    let now = Utc::now();

    // Filter to active or recently expired statuses
    let mut statuses: Vec<_> = latest
        .into_values()
        .filter(|e| e.is_valid() || e.is_recently_expired())
        .collect();

    statuses.sort_by(|a, b| b.ts.cmp(&a.ts));

    #[derive(serde::Serialize)]
    struct StatusInfo {
        agent: String,
        message: String,
        active: bool,
        expires_in_secs: i64,
    }

    let infos: Vec<StatusInfo> = statuses
        .iter()
        .map(|s| StatusInfo {
            agent: s.agent.clone(),
            message: s.message.clone(),
            active: s.is_valid(),
            expires_in_secs: (s.expires_at - now).num_seconds(),
        })
        .collect();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&infos)?);
        }
        OutputFormat::Toon => {
            if infos.is_empty() {
                println!("statuses: []");
            } else {
                println!("{}", to_toon_list(&infos));
            }
        }
        OutputFormat::Text => {
            if statuses.is_empty() {
                println!("No active statuses.");
            } else {
                for s in &statuses {
                    let indicator = if s.is_valid() {
                        "●".green()
                    } else {
                        "●".dimmed()
                    };
                    println!("{} {} {}", indicator, s.agent.cyan(), s.message.dimmed());
                }
            }
        }
    }

    Ok(())
}

fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        format!("{}h", h)
    } else if secs >= 60 {
        let m = secs / 60;
        format!("{}m", m)
    } else {
        format!("{}s", secs)
    }
}
