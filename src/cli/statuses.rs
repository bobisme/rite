use anyhow::{Context, Result, bail};
use chrono::Utc;
use colored::Colorize;

use super::OutputFormat;
use crate::core::identity::require_agent;
use crate::core::project::statuses_path;
use crate::core::status::AgentStatusEntry;
use crate::storage::jsonl::{append_record, read_records};

const MAX_STATUS_MESSAGE_CHARS: usize = 32;

/// Parse a TTL string like "1h", "30m", "8h", "3600" into seconds.
fn parse_ttl(ttl: &str) -> Result<u64> {
    let ttl = ttl.trim();

    if ttl.is_empty() {
        bail!("TTL cannot be empty");
    }

    let (digits, multiplier) = if let Some(hours) = ttl.strip_suffix('h') {
        (hours, 3600u64)
    } else if let Some(mins) = ttl.strip_suffix('m') {
        (mins, 60u64)
    } else if let Some(secs) = ttl.strip_suffix('s') {
        (secs, 1u64)
    } else {
        (ttl, 1u64)
    };

    if digits.is_empty() {
        bail!("TTL missing numeric value");
    }

    let value = digits.parse::<u64>()?;
    let seconds = value
        .checked_mul(multiplier)
        .with_context(|| "TTL is too large")?;

    if seconds > i64::MAX as u64 {
        bail!("TTL is too large");
    }

    Ok(seconds)
}

fn truncate_status_message(message: &str) -> String {
    message.chars().take(MAX_STATUS_MESSAGE_CHARS).collect()
}

pub fn set(message: &str, ttl: &str, agent: Option<&str>, _format: OutputFormat) -> Result<()> {
    let agent_name = require_agent(agent)?;

    let message = truncate_status_message(message);

    let ttl_secs = parse_ttl(ttl)?;
    let entry = AgentStatusEntry::new(&agent_name, &message, ttl_secs);
    append_record(&statuses_path(), &entry)?;

    println!(
        "{} Status set for {}",
        "Success:".green(),
        format_duration(ttl_secs)
    );
    println!("  {} {}", agent_name.cyan(), message.dimmed());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ttl_handles_supported_units() {
        assert_eq!(parse_ttl("30s").unwrap(), 30);
        assert_eq!(parse_ttl("15m").unwrap(), 900);
        assert_eq!(parse_ttl("2h").unwrap(), 7200);
        assert_eq!(parse_ttl("45").unwrap(), 45);
    }

    #[test]
    fn parse_ttl_rejects_empty_and_overflowing_values() {
        assert!(parse_ttl("").is_err());
        assert!(parse_ttl("h").is_err());
        assert!(parse_ttl(&format!("{}h", u64::MAX)).is_err());
        assert!(parse_ttl(&(i64::MAX as u64 + 1).to_string()).is_err());
    }

    #[test]
    fn truncate_status_message_uses_char_boundaries() {
        let message = "🙂".repeat(MAX_STATUS_MESSAGE_CHARS + 4);
        let truncated = truncate_status_message(&message);

        assert_eq!(truncated.chars().count(), MAX_STATUS_MESSAGE_CHARS);
        assert!(truncated.is_char_boundary(truncated.len()));
    }
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

    statuses.sort_by_key(|status| std::cmp::Reverse(status.ts));

    #[derive(serde::Serialize)]
    struct StatusInfo {
        agent: String,
        message: String,
        active: bool,
        expires_in_secs: i64,
    }

    #[derive(serde::Serialize)]
    struct StatusesOutput {
        statuses: Vec<StatusInfo>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        advice: Vec<String>,
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
            let output = StatusesOutput {
                statuses: infos,
                advice: vec![],
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Pretty => {
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
        OutputFormat::Text => {
            for s in &statuses {
                let time_ago = format_time_ago(s.ts);
                println!("{}  \"{}\"  {}", s.agent, s.message, time_ago);
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

fn format_time_ago(ts: chrono::DateTime<Utc>) -> String {
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
