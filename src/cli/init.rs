use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::fs;
use std::path::Path;

use crate::core::project::{
    agents_path, botbus_dir, channels_dir, claims_path, state_path, BOTBUS_DIR,
};
use crate::storage::State;

/// Initialize BotBus in the current directory.
pub fn run(force: bool, project_root: &Path) -> Result<()> {
    let botbus = botbus_dir(project_root);

    // Check if already initialized
    if botbus.exists() {
        if force {
            println!(
                "{} Removing existing .botbus directory...",
                "Warning:".yellow()
            );
            fs::remove_dir_all(&botbus)
                .with_context(|| format!("Failed to remove existing {}", botbus.display()))?;
        } else {
            bail!(
                "BotBus already initialized in this directory.\n\
                 Use --force to reinitialize."
            );
        }
    }

    // Create directory structure
    fs::create_dir_all(channels_dir(project_root))
        .with_context(|| "Failed to create .botbus/channels directory")?;

    // Create empty agents.jsonl
    fs::write(agents_path(project_root), "").with_context(|| "Failed to create agents.jsonl")?;

    // Create empty claims.jsonl
    fs::write(claims_path(project_root), "").with_context(|| "Failed to create claims.jsonl")?;

    // Create default state.json
    let state = State::default();
    let state_json = serde_json::to_string_pretty(&state)?;
    fs::write(state_path(project_root), state_json)
        .with_context(|| "Failed to create state.json")?;

    // Add index.sqlite to .gitignore if in a git repo
    add_to_gitignore(project_root)?;

    println!(
        "{} Initialized BotBus in {}",
        "Success:".green(),
        botbus.display()
    );
    println!("\nNext steps:");
    println!("  {} Register an agent identity", "botbus register".cyan());
    println!(
        "  {} Send a message",
        "botbus send general \"Hello!\"".cyan()
    );

    Ok(())
}

/// Add .botbus/index.sqlite to .gitignore if in a git repo.
fn add_to_gitignore(project_root: &Path) -> Result<()> {
    let git_dir = project_root.join(".git");
    if !git_dir.exists() {
        return Ok(());
    }

    let gitignore_path = project_root.join(".gitignore");
    let entry = format!("{}index.sqlite", BOTBUS_DIR.to_owned() + "/");

    // Read existing .gitignore content
    let existing = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path).unwrap_or_default()
    } else {
        String::new()
    };

    // Check if already present
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }

    // Append the entry
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("# BotBus index (generated)\n{}\n", entry));

    fs::write(&gitignore_path, content).with_context(|| "Failed to update .gitignore")?;

    println!("{} Added {} to .gitignore", "Info:".blue(), entry);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_creates_structure() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        run(false, root).unwrap();

        assert!(botbus_dir(root).exists());
        assert!(channels_dir(root).exists());
        assert!(agents_path(root).exists());
        assert!(claims_path(root).exists());
        assert!(state_path(root).exists());
    }

    #[test]
    fn test_init_fails_if_exists() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        run(false, root).unwrap();
        let result = run(false, root);

        assert!(result.is_err());
    }

    #[test]
    fn test_init_force_reinitializes() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        run(false, root).unwrap();

        // Add a file to channels
        fs::write(channels_dir(root).join("test.jsonl"), "test").unwrap();

        // Force reinitialize
        run(true, root).unwrap();

        // The test file should be gone
        assert!(!channels_dir(root).join("test.jsonl").exists());
    }

    #[test]
    fn test_gitignore_update() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create a fake git repo
        fs::create_dir(root.join(".git")).unwrap();

        run(false, root).unwrap();

        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".botbus/index.sqlite"));
    }
}
