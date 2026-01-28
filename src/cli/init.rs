//! Initialize BotBus data directory.

use anyhow::Result;
use colored::Colorize;

use crate::core::project::{data_dir, ensure_data_dir};

/// Initialize the BotBus data directory.
///
/// This creates the global data directory structure if it doesn't exist.
/// Safe to run multiple times - will just report the existing path.
pub fn run() -> Result<()> {
    let path = data_dir();
    let existed = path.exists();

    ensure_data_dir()?;

    if existed {
        println!("{} BotBus data directory already exists", "✓".green());
    } else {
        println!("{} Created BotBus data directory", "✓".green());
    }

    println!("  {}", path.display().to_string().cyan());

    println!();
    println!("Next steps:");
    println!(
        "  {} Set your agent identity",
        "export BOTBUS_AGENT=$(botbus generate-name)".cyan()
    );
    println!(
        "  {} Send a message",
        "botbus send general \"Hello!\"".cyan()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::project::{DATA_DIR_ENV_VAR, channels_dir, claims_path, state_path};
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn test_init_creates_structure() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        // Override data dir for test
        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
        }

        run().unwrap();

        assert!(data_dir().exists());
        assert!(channels_dir().exists());
        assert!(claims_path().parent().unwrap().exists());
        assert!(state_path().parent().unwrap().exists());

        // Cleanup
        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
        }
    }

    #[test]
    #[serial]
    fn test_init_idempotent() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
        }

        // Run twice - should succeed both times
        run().unwrap();
        run().unwrap();

        assert!(data_dir().exists());

        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
        }
    }
}
