//! Git operations for BotBus data directory sync.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// Check if git is available on the system.
pub fn check_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if the data directory is a git repository.
pub fn is_git_repo(data_dir: &Path) -> bool {
    data_dir.join(".git").exists()
}

/// Initialize a git repository in the data directory.
pub fn init_repo(data_dir: &Path, remote_url: Option<&str>) -> Result<()> {
    if !check_git_available() {
        bail!("git is not installed or not in PATH");
    }

    if is_git_repo(data_dir) {
        bail!("Git repository already exists in {}", data_dir.display());
    }

    // Initialize git repo
    let status = Command::new("git")
        .current_dir(data_dir)
        .arg("init")
        .status()
        .context("Failed to run git init")?;

    if !status.success() {
        bail!("git init failed");
    }

    // Create .gitattributes (union merge for JSONL)
    let gitattributes = data_dir.join(".gitattributes");
    std::fs::write(
        &gitattributes,
        "# Union merge for append-only JSONL\n\
         *.jsonl merge=union\n\
         \n\
         # Binary files (don't merge)\n\
         *.db binary\n\
         *.db-wal binary\n\
         *.db-shm binary\n\
         \n\
         # Attachments (future: git-annex or reference-only)\n\
         attachments/** binary\n",
    )
    .context("Failed to create .gitattributes")?;

    // Create .gitignore
    let gitignore = data_dir.join(".gitignore");
    std::fs::write(
        &gitignore,
        "# SQLite indexes (derived from JSONL)\n\
         *.db\n\
         *.db-wal\n\
         *.db-shm\n\
         \n\
         # Local state (machine-specific)\n\
         state.json\n\
         \n\
         # Attachments (synced separately, or reference-only)\n\
         attachments/\n\
         \n\
         # Temp files\n\
         *.tmp\n\
         *.lock\n",
    )
    .context("Failed to create .gitignore")?;

    // Add and commit .gitattributes and .gitignore
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["add", ".gitattributes", ".gitignore"])
        .status()
        .context("Failed to add git config files")?;

    if !status.success() {
        bail!("git add failed for config files");
    }

    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["commit", "-m", "chore: initialize botbus data repo"])
        .status()
        .context("Failed to commit git config files")?;

    if !status.success() {
        bail!("git commit failed for config files");
    }

    // Add any existing JSONL files
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["add", "*.jsonl", "channels/*.jsonl"])
        .status();

    // It's OK if this fails (no JSONL files yet)
    if status.is_ok() && status.unwrap().success() {
        // Commit existing data if any
        let status = Command::new("git")
            .current_dir(data_dir)
            .args(["commit", "-m", "chore: add existing botbus data"])
            .status();

        // It's OK if this fails (nothing to commit)
        let _ = status;
    }

    // Add remote if provided
    if let Some(url) = remote_url {
        let status = Command::new("git")
            .current_dir(data_dir)
            .args(["remote", "add", "origin", url])
            .status()
            .context("Failed to add git remote")?;

        if !status.success() {
            bail!("git remote add failed");
        }

        // Try to push to remote (create main branch on remote)
        let status = Command::new("git")
            .current_dir(data_dir)
            .args(["push", "-u", "origin", "main"])
            .status()
            .context("Failed to push to remote")?;

        if !status.success() {
            eprintln!(
                "Warning: Failed to push to remote. You may need to run 'bus sync --push' manually."
            );
        }
    }

    Ok(())
}

/// Commit specific files with a message.
pub fn commit_files(data_dir: &Path, files: &[&str], message: &str) -> Result<()> {
    if !is_git_repo(data_dir) {
        // Silent skip if not a git repo
        return Ok(());
    }

    // Add files
    let mut cmd = Command::new("git");
    cmd.current_dir(data_dir).arg("add");
    for file in files {
        cmd.arg(file);
    }

    let status = cmd.status();
    if status.is_err() || !status.unwrap().success() {
        // Log warning but don't fail
        eprintln!("Warning: git add failed (auto-commit)");
        return Ok(());
    }

    // Commit
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["commit", "-m", message])
        .status();

    if status.is_err() || !status.unwrap().success() {
        // Log warning but don't fail (might be nothing to commit)
        // This is expected if the file hasn't changed
        return Ok(());
    }

    Ok(())
}

/// Push local commits to remote.
pub fn push(data_dir: &Path) -> Result<()> {
    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'bus sync init' first.");
    }

    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["push", "origin", "main"])
        .status()
        .context("Failed to run git push")?;

    if !status.success() {
        bail!("git push failed. Check your network connection and remote configuration.");
    }

    Ok(())
}

/// Pull and merge changes from remote.
pub fn pull(data_dir: &Path) -> Result<()> {
    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'bus sync init' first.");
    }

    // Fetch from remote
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["fetch", "origin"])
        .status()
        .context("Failed to run git fetch")?;

    if !status.success() {
        bail!("git fetch failed. Check your network connection.");
    }

    // Merge with union strategy (configured in .gitattributes)
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["merge", "origin/main"])
        .status()
        .context("Failed to run git merge")?;

    if !status.success() {
        bail!("git merge failed. You may need to resolve conflicts manually.");
    }

    Ok(())
}

/// Get git status (staged, unstaged, ahead/behind).
pub fn status(data_dir: &Path) -> Result<String> {
    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'bus sync init' first.");
    }

    // Get short status
    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["status", "--short", "--branch"])
        .output()
        .context("Failed to run git status")?;

    if !output.status.success() {
        bail!("git status failed");
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_check_git_available() {
        // This test assumes git is installed (required for BotBus development)
        assert!(check_git_available());
    }

    #[test]
    fn test_is_git_repo() {
        let temp = TempDir::new().unwrap();
        assert!(!is_git_repo(temp.path()));

        // Create .git directory
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        assert!(is_git_repo(temp.path()));
    }

    #[test]
    fn test_init_repo() {
        if !check_git_available() {
            eprintln!("Skipping test_init_repo: git not available");
            return;
        }

        let temp = TempDir::new().unwrap();

        // Initialize repo
        init_repo(temp.path(), None).unwrap();

        // Check that .git exists
        assert!(is_git_repo(temp.path()));

        // Check that .gitattributes exists
        assert!(temp.path().join(".gitattributes").exists());

        // Check that .gitignore exists
        assert!(temp.path().join(".gitignore").exists());

        // Try to init again - should fail
        let result = init_repo(temp.path(), None);
        assert!(result.is_err());
    }
}
