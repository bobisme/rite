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
        .args([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "chore: initialize botbus data repo",
        ])
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
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "chore: add existing botbus data",
            ])
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

    // Commit (disable GPG signing to avoid interactive prompts)
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["-c", "commit.gpgsign=false", "commit", "-m", message])
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
    if !check_git_available() {
        bail!("git is not installed or not in PATH. Please install git to use sync features.");
    }

    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'bus sync init' first.");
    }

    // Check if remote is configured
    let remote_check = Command::new("git")
        .current_dir(data_dir)
        .args(["remote", "get-url", "origin"])
        .output();

    if remote_check.is_err() || !remote_check.as_ref().unwrap().status.success() {
        bail!(
            "No remote configured. Add a remote with: cd {} && git remote add origin <url>",
            data_dir.display()
        );
    }

    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["push", "origin", "main"])
        .output()
        .context("Failed to run git push")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Provide helpful error messages based on git output
        if stderr.contains("Could not resolve host") || stderr.contains("unable to access") {
            bail!(
                "Network error: Could not reach remote server. Check your internet connection and try again."
            );
        } else if stderr.contains("authentication failed") || stderr.contains("Permission denied") {
            bail!("Authentication failed. Check your credentials or SSH keys.");
        } else if stderr.contains("rejected") {
            bail!("Push rejected. Try pulling first with 'bus sync pull'.");
        } else {
            bail!("git push failed: {}", stderr.trim());
        }
    }

    Ok(())
}

/// Pull and merge changes from remote.
///
/// Returns true if changes were pulled and merged, false if already up to date.
pub fn pull(data_dir: &Path) -> Result<bool> {
    if !check_git_available() {
        bail!("git is not installed or not in PATH. Please install git to use sync features.");
    }

    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'bus sync init' first.");
    }

    // Check if remote is configured
    let remote_check = Command::new("git")
        .current_dir(data_dir)
        .args(["remote", "get-url", "origin"])
        .output();

    if remote_check.is_err() || !remote_check.as_ref().unwrap().status.success() {
        bail!(
            "No remote configured. Add a remote with: cd {} && git remote add origin <url>",
            data_dir.display()
        );
    }

    // Fetch from remote
    let fetch_output = Command::new("git")
        .current_dir(data_dir)
        .args(["fetch", "origin"])
        .output()
        .context("Failed to run git fetch")?;

    if !fetch_output.status.success() {
        let stderr = String::from_utf8_lossy(&fetch_output.stderr);

        if stderr.contains("Could not resolve host") || stderr.contains("unable to access") {
            bail!(
                "Network error: Could not reach remote server. Check your internet connection and try again."
            );
        } else if stderr.contains("authentication failed") || stderr.contains("Permission denied") {
            bail!("Authentication failed. Check your credentials or SSH keys.");
        } else {
            bail!("git fetch failed: {}", stderr.trim());
        }
    }

    // Merge with union strategy (configured in .gitattributes)
    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["merge", "origin/main"])
        .output()
        .context("Failed to run git merge")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        if stderr.contains("CONFLICT") || stdout.contains("CONFLICT") {
            // Abort the merge to leave repo in clean state
            let _ = Command::new("git")
                .current_dir(data_dir)
                .args(["merge", "--abort"])
                .status();

            bail!(
                "Merge conflict detected. The merge has been aborted.\nPlease resolve conflicts manually:\n  cd {}\n  git merge origin/main\n  # resolve conflicts\n  git commit",
                data_dir.display()
            );
        } else {
            bail!("git merge failed: {}", stderr.trim());
        }
    }

    // Check if merge actually changed anything
    // If output contains "Already up to date", no changes were pulled
    let output_str = String::from_utf8_lossy(&output.stdout);
    let changed =
        !output_str.contains("Already up to date") && !output_str.contains("Already up-to-date");

    Ok(changed)
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

/// Get detailed status info (uncommitted changes, ahead/behind, remote).
#[derive(Debug, serde::Serialize)]
pub struct StatusInfo {
    pub uncommitted_changes: usize,
    pub ahead: usize,
    pub behind: usize,
    pub remote_url: Option<String>,
    pub is_git_repo: bool,
    pub has_conflicts: bool,
}

pub fn get_status_info(data_dir: &Path) -> Result<StatusInfo> {
    if !is_git_repo(data_dir) {
        return Ok(StatusInfo {
            uncommitted_changes: 0,
            ahead: 0,
            behind: 0,
            remote_url: None,
            is_git_repo: false,
            has_conflicts: false,
        });
    }

    // Count uncommitted changes
    let status_output = Command::new("git")
        .current_dir(data_dir)
        .args(["status", "--short"])
        .output()
        .context("Failed to run git status")?;

    let uncommitted_changes = String::from_utf8_lossy(&status_output.stdout)
        .lines()
        .count();

    // Check for merge conflicts
    let has_conflicts = String::from_utf8_lossy(&status_output.stdout)
        .lines()
        .any(|line| line.starts_with("UU ") || line.starts_with("AA ") || line.starts_with("DD "));

    // Get ahead/behind counts
    let rev_list_output = Command::new("git")
        .current_dir(data_dir)
        .args(["rev-list", "--left-right", "--count", "origin/main...HEAD"])
        .output();

    let (behind, ahead) = if let Ok(output) = rev_list_output {
        let output_str = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = output_str.split_whitespace().collect();
        if parts.len() == 2 {
            (parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0))
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    // Get remote URL
    let remote_output = Command::new("git")
        .current_dir(data_dir)
        .args(["remote", "get-url", "origin"])
        .output();

    let remote_url = if let Ok(output) = remote_output
        && output.status.success()
    {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() { None } else { Some(url) }
    } else {
        None
    };

    Ok(StatusInfo {
        uncommitted_changes,
        ahead,
        behind,
        remote_url,
        is_git_repo: true,
        has_conflicts,
    })
}

/// Get recent git log entries.
#[derive(Debug, serde::Serialize)]
pub struct LogEntry {
    pub hash: String,
    pub date: String,
    pub message: String,
}

pub fn get_log(data_dir: &Path, count: usize) -> Result<Vec<LogEntry>> {
    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'bus sync init' first.");
    }

    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["log", &format!("-n{}", count), "--pretty=format:%h|%ai|%s"])
        .output()
        .context("Failed to run git log")?;

    if !output.status.success() {
        bail!("git log failed");
    }

    let log_output = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<LogEntry> = log_output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() == 3 {
                Some(LogEntry {
                    hash: parts[0].to_string(),
                    date: parts[1].to_string(),
                    message: parts[2].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(entries)
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
