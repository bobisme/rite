//! Global path helpers for BotBus data storage.
//!
//! BotBus uses XDG Base Directory specification for storage:
//! - Data: `$XDG_DATA_HOME/botbus/` (default: `~/.local/share/botbus/`)
//!
//! For testing, set `BOTBUS_DATA_DIR` to override the data directory.
//!
//! Directory structure:
//! ```text
//! ~/.local/share/botbus/
//!   channels/
//!     general.jsonl
//!     <channel>.jsonl
//!   claims.jsonl
//!   state.json
//!   index.sqlite
//! ```

use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::env;
use std::fs;
use std::path::PathBuf;

/// Environment variable to override the data directory (for testing).
pub const DATA_DIR_ENV_VAR: &str = "BOTBUS_DATA_DIR";

/// Get the BotBus data directory.
///
/// Checks in order:
/// 1. `BOTBUS_DATA_DIR` environment variable (for testing)
/// 2. XDG data directory (`$XDG_DATA_HOME/botbus/`)
/// 3. Fallback: `~/.local/share/botbus/`
///
/// Note: This function reads the env var each time, so tests can set different
/// values for different test cases.
pub fn data_dir() -> PathBuf {
    // 1. Check env var override (for testing)
    if let Ok(dir) = env::var(DATA_DIR_ENV_VAR) {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }

    // 2. Try XDG-compliant path
    if let Some(proj_dirs) = ProjectDirs::from("", "", "botbus") {
        return proj_dirs.data_dir().to_path_buf();
    }

    // 3. Fallback: ~/.local/share/botbus/
    if let Some(user_dirs) = directories::UserDirs::new() {
        return user_dirs
            .home_dir()
            .join(".local")
            .join("share")
            .join("botbus");
    }

    // Last resort: current directory (not ideal, but won't panic)
    PathBuf::from(".botbus")
}

/// Ensure the data directory and subdirectories exist.
///
/// Creates:
/// - Data directory
/// - Channels subdirectory
pub fn ensure_data_dir() -> Result<PathBuf> {
    let dir = data_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create data dir: {}", dir.display()))?;
    fs::create_dir_all(channels_dir()).with_context(|| {
        format!(
            "Failed to create channels dir: {}",
            channels_dir().display()
        )
    })?;
    Ok(dir)
}

/// Get the channels directory path.
pub fn channels_dir() -> PathBuf {
    data_dir().join("channels")
}

/// Get the path to a specific channel file.
pub fn channel_path(channel: &str) -> PathBuf {
    channels_dir().join(format!("{}.jsonl", channel))
}

/// Get the claims.jsonl path.
pub fn claims_path() -> PathBuf {
    data_dir().join("claims.jsonl")
}

/// Get the state.json path.
pub fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

/// Get the index.sqlite path.
pub fn index_path() -> PathBuf {
    data_dir().join("index.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_data_dir_not_empty() {
        let dir = data_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn test_data_dir_override() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        // Set override
        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
        }

        let dir = data_dir();
        assert_eq!(dir, temp.path());

        // Clean up
        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
        }
    }

    #[test]
    fn test_channels_dir_is_subdir() {
        let base = data_dir();
        let channels = channels_dir();
        assert!(channels.starts_with(&base));
        assert!(channels.ends_with("channels"));
    }

    #[test]
    fn test_channel_path() {
        let base = data_dir();
        let path = channel_path("general");
        assert!(path.ends_with("general.jsonl"));
        assert!(path.starts_with(&base));
    }

    #[test]
    fn test_claims_path() {
        let base = data_dir();
        let path = claims_path();
        assert!(path.ends_with("claims.jsonl"));
        assert!(path.starts_with(&base));
    }

    #[test]
    fn test_state_path() {
        let base = data_dir();
        let path = state_path();
        assert!(path.ends_with("state.json"));
        assert!(path.starts_with(&base));
    }

    #[test]
    fn test_index_path() {
        let base = data_dir();
        let path = index_path();
        assert!(path.ends_with("index.sqlite"));
        assert!(path.starts_with(&base));
    }
}
