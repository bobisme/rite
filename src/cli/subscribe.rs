//! Channel subscription commands.

use anyhow::Result;
use colored::Colorize;

use crate::core::identity::require_agent;
use crate::core::project::data_dir;
use crate::storage::agent_state::AgentStateManager;

/// Subscribe to a channel.
pub fn subscribe(channel: String, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;
    let manager = AgentStateManager::new(&data_dir(), &agent);

    let added = manager.subscribe(&channel)?;

    if added {
        println!(
            "{} Subscribed to #{}",
            "✓".green(),
            channel.cyan()
        );
    } else {
        println!(
            "{} Already subscribed to #{}",
            "ℹ".blue(),
            channel.cyan()
        );
    }

    Ok(())
}

/// Unsubscribe from a channel.
pub fn unsubscribe(channel: String, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;
    let manager = AgentStateManager::new(&data_dir(), &agent);

    let removed = manager.unsubscribe(&channel)?;

    if removed {
        println!(
            "{} Unsubscribed from #{}",
            "✓".green(),
            channel.cyan()
        );
    } else {
        println!(
            "{} Not subscribed to #{}",
            "ℹ".blue(),
            channel.cyan()
        );
    }

    Ok(())
}

/// List subscribed channels.
pub fn list_subscriptions(explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;
    let manager = AgentStateManager::new(&data_dir(), &agent);

    let channels = manager.get_subscribed_channels()?;

    if channels.is_empty() {
        println!("{} No channel subscriptions", "ℹ".blue());
    } else {
        println!("{} Subscribed channels:", "→".cyan());
        for channel in channels {
            println!("  #{}", channel.cyan());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Integration tests moved to tests/integration/ since they require
    // global data directory mocking
}
