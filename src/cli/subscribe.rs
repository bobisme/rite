//! Channel subscription commands.

use anyhow::{Result, bail};
use colored::Colorize;

use crate::core::channel::is_valid_channel_name;
use crate::core::identity::require_agent;
use crate::core::project::data_dir;
use crate::storage::agent_state::AgentStateManager;

fn normalize_channel(channel: &str) -> Result<&str> {
    let channel = channel.strip_prefix('#').unwrap_or(channel);
    if !is_valid_channel_name(channel) {
        bail!(
            "Invalid channel name: '{}'\n\n\
             Channel names must be lowercase alphanumeric with hyphens.",
            channel
        );
    }
    Ok(channel)
}

/// Subscribe to a channel.
pub fn subscribe(channel: String, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;
    let manager = AgentStateManager::new(&data_dir(), &agent);

    let channel = normalize_channel(&channel)?;

    let added = manager.subscribe(channel)?;

    if added {
        println!("{} Subscribed to #{}", "✓".green(), channel.cyan());
    } else {
        println!("{} Already subscribed to #{}", "ℹ".blue(), channel.cyan());
    }

    Ok(())
}

/// Unsubscribe from a channel.
pub fn unsubscribe(channel: String, explicit_agent: Option<&str>) -> Result<()> {
    let agent = require_agent(explicit_agent)?;
    let manager = AgentStateManager::new(&data_dir(), &agent);

    let channel = normalize_channel(&channel)?;

    let removed = manager.unsubscribe(channel)?;

    if removed {
        println!("{} Unsubscribed from #{}", "✓".green(), channel.cyan());
    } else {
        println!("{} Not subscribed to #{}", "ℹ".blue(), channel.cyan());
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
    use super::*;
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
    fn normalize_channel_strips_hash_prefix() {
        assert_eq!(normalize_channel("#general").unwrap(), "general");
    }

    #[test]
    fn normalize_channel_rejects_invalid_names() {
        assert!(normalize_channel("Uppercase").is_err());
        assert!(normalize_channel("has space").is_err());
        assert!(normalize_channel("#").is_err());
    }

    #[test]
    #[serial]
    fn subscribe_rejects_invalid_channel_without_persisting_state() {
        let _env = TestEnv::new();

        let result = subscribe("Bad Channel".to_string(), Some("test-agent"));

        assert!(result.is_err());
        let manager = AgentStateManager::new(&data_dir(), "test-agent");
        assert!(manager.get_subscribed_channels().unwrap().is_empty());
    }

    #[test]
    #[serial]
    fn subscribe_stores_normalized_channel_name() {
        let _env = TestEnv::new();

        subscribe("#general".to_string(), Some("test-agent")).unwrap();

        let manager = AgentStateManager::new(&data_dir(), "test-agent");
        assert_eq!(
            manager.get_subscribed_channels().unwrap(),
            vec!["general".to_string()]
        );
    }
}
