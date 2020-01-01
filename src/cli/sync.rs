//! CLI commands for git sync.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use tracing::warn;

use super::OutputFormat;
use crate::core::project::data_dir;
use crate::sync::git;

/// Initialize git repository in data directory.
pub fn init(remote_url: Option<String>) -> Result<()> {
    let dir = data_dir();

    println!(
        "{} Initializing git repository in {}",
        "Init:".cyan(),
        dir.display()
    );

    git::init_repo(&dir, remote_url.as_deref())?;

    println!("{} Git repository initialized", "Success:".green());
    println!();
    println!("Created files:");
    println!("  - .gitattributes (union merge for *.jsonl)");
    println!("  - .gitignore (*.db, state.json, attachments/)");

    if let Some(url) = remote_url {
        println!();
        println!("Remote configured:");
        println!("  origin: {}", url.cyan());
        println!();
        println!("Next steps:");
        println!("  - Use 'bus sync --push' to push changes");
        println!("  - Use 'bus sync --pull' to pull changes");
        println!("  - Messages, claims, and releases will auto-commit");
    } else {
        println!();
        println!("No remote configured. To add one:");
        println!("  cd {}", dir.display());
        println!("  git remote add origin <url>");
        println!("  git push -u origin main");
    }

    Ok(())
}

/// Push local commits to remote.
pub fn push() -> Result<()> {
    let dir = data_dir();

    println!("{} Pushing to remote...", "Sync:".cyan());

    git::push(&dir)?;

    println!("{} Pushed to remote", "Success:".green());

    Ok(())
}

/// Pull and merge changes from remote.
pub fn pull() -> Result<()> {
    let dir = data_dir();

    println!("{} Pulling from remote...", "Sync:".cyan());

    let changed = git::pull(&dir)?;

    println!("{} Pulled from remote", "Success:".green());

    // Auto-rebuild index if JSONL files changed
    if changed {
        println!();
        println!("{} Rebuilding search index...", "Sync:".cyan());

        match crate::index::IndexSyncer::new() {
            Ok(mut syncer) => match syncer.rebuild() {
                Ok(stats) => {
                    println!("{} Index rebuilt", "Success:".green());
                    println!("  - Channels synced: {}", stats.channels_synced);
                    println!("  - Messages indexed: {}", stats.messages_indexed);

                    if !stats.errors.is_empty() {
                        println!();
                        println!("{} Index errors:", "Warning:".yellow());
                        for err in &stats.errors {
                            println!("  - {}", err);
                        }
                    }
                }
                Err(error) => {
                    warn!(%error, "failed to rebuild index after sync pull");
                    eprintln!("  You can manually rebuild with: bus index rebuild");
                }
            },
            Err(error) => {
                warn!(%error, "failed to open index after sync pull");
                eprintln!("  You can manually rebuild with: bus index rebuild");
            }
        }
    }

    Ok(())
}

/// Show git status.
pub fn status(format: OutputFormat) -> Result<()> {
    let dir = data_dir();

    let info = git::get_status_info(&dir)?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            if !info.is_git_repo {
                println!("{}", "Git Status:".bold());
                println!();
                println!("{} Not a git repository", "Status:".yellow());
                println!("  Run: bus sync init");
                return Ok(());
            }

            println!("{}", "Git Status:".bold());
            println!();

            // Remote info
            if let Some(ref url) = info.remote_url {
                println!("{} {}", "Remote:".cyan(), url);
            } else {
                println!("{} Not configured", "Remote:".yellow());
            }

            // Uncommitted changes
            if info.uncommitted_changes > 0 {
                println!(
                    "{} {} file(s)",
                    "Uncommitted:".yellow(),
                    info.uncommitted_changes
                );
            } else {
                println!("{} None", "Uncommitted:".green());
            }

            // Ahead/behind
            if info.ahead > 0 || info.behind > 0 {
                println!(
                    "{} {} ahead, {} behind",
                    "Sync:".cyan(),
                    info.ahead,
                    info.behind
                );

                if info.ahead > 0 {
                    println!("  → Run: bus sync push");
                }
                if info.behind > 0 {
                    println!("  → Run: bus sync pull");
                }
            } else if info.remote_url.is_some() {
                println!("{} Up to date with remote", "Sync:".green());
            }

            // Conflicts
            if info.has_conflicts {
                println!("{} Merge conflicts detected!", "Conflicts:".red().bold());
            }
        }
    }

    Ok(())
}

/// Show recent git log entries.
pub fn log(count: usize, format: OutputFormat) -> Result<()> {
    let dir = data_dir();

    let entries = git::get_log(&dir, count)?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            println!("{}", "Recent Sync Commits:".bold());
            println!();

            if entries.is_empty() {
                println!("No commits yet");
                return Ok(());
            }

            for entry in entries {
                println!(
                    "{} {} {}",
                    entry.hash.yellow(),
                    entry.date.dimmed(),
                    entry.message
                );
            }
        }
    }

    Ok(())
}

/// Check sync repository health.
#[derive(Debug, Serialize)]
pub struct SyncCheckResult {
    pub is_git_repo: bool,
    pub git_available: bool,
    pub uncommitted_changes: usize,
    pub has_conflicts: bool,
    pub remote_configured: bool,
    pub index_up_to_date: bool,
    pub ahead: usize,
    pub behind: usize,
    pub healthy: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<String>,
}

pub fn check(format: OutputFormat) -> Result<()> {
    let dir = data_dir();

    let git_available = git::check_git_available();
    let is_git_repo = git::is_git_repo(&dir);

    let info = if is_git_repo {
        git::get_status_info(&dir)?
    } else {
        git::StatusInfo {
            uncommitted_changes: 0,
            ahead: 0,
            behind: 0,
            remote_url: None,
            is_git_repo: false,
            has_conflicts: false,
        }
    };

    // Check if index needs rebuild (reuse logic from index.rs)
    let index_up_to_date = check_index_up_to_date();

    // Build advice based on the check results
    let mut advice = Vec::new();
    if !git_available {
        // No git installed
    } else if !is_git_repo {
        advice.push("bus sync init".to_string());
    } else if info.has_conflicts {
        // Has conflicts, needs manual resolution
    } else if info.ahead > 0 {
        advice.push("bus sync push".to_string());
    } else if info.behind > 0 {
        advice.push("bus sync pull".to_string());
    }

    let result = SyncCheckResult {
        is_git_repo,
        git_available,
        uncommitted_changes: info.uncommitted_changes,
        has_conflicts: info.has_conflicts,
        remote_configured: info.remote_url.is_some(),
        index_up_to_date,
        ahead: info.ahead,
        behind: info.behind,
        healthy: git_available
            && is_git_repo
            && !info.has_conflicts
            && info.remote_url.is_some()
            && index_up_to_date,
        advice,
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        OutputFormat::Pretty | OutputFormat::Text => {
            println!("{}", "Sync Health Check:".bold());
            println!();

            // Git availability
            if result.git_available {
                println!("{} Git is installed", "✓".green());
            } else {
                println!("{} Git is not installed", "✗".red());
                println!("  → Install git to use sync features");
            }

            // Git repo initialized
            if result.is_git_repo {
                println!("{} Git repository initialized", "✓".green());
            } else {
                println!("{} Git repository not initialized", "✗".yellow());
                println!("  → Run: bus sync init");
            }

            if !result.is_git_repo {
                return Ok(());
            }

            // Remote configured
            if result.remote_configured {
                println!("{} Remote configured", "✓".green());
            } else {
                println!("{} No remote configured", "!".yellow());
                println!(
                    "  → Add remote: cd {} && git remote add origin <url>",
                    dir.display()
                );
            }

            // Uncommitted changes
            if result.uncommitted_changes == 0 {
                println!("{} No uncommitted changes", "✓".green());
            } else {
                println!(
                    "{} {} uncommitted file(s)",
                    "!".yellow(),
                    result.uncommitted_changes
                );
            }

            // Conflicts
            if result.has_conflicts {
                println!("{} Merge conflicts detected!", "✗".red().bold());
                println!("  → Resolve conflicts: cd {} && git status", dir.display());
            } else {
                println!("{} No merge conflicts", "✓".green());
            }

            // Ahead/behind
            if result.ahead > 0 {
                println!("{} {} commits ahead of remote", "!".yellow(), result.ahead);
                println!("  → Run: bus sync push");
            }
            if result.behind > 0 {
                println!("{} {} commits behind remote", "!".yellow(), result.behind);
                println!("  → Run: bus sync pull");
            }
            if result.ahead == 0 && result.behind == 0 && result.remote_configured {
                println!("{} In sync with remote", "✓".green());
            }

            // Index status
            if result.index_up_to_date {
                println!("{} Search index is up to date", "✓".green());
            } else {
                println!("{} Search index needs rebuild", "!".yellow());
                println!("  → Run: bus index rebuild --if-needed");
            }

            println!();
            if result.healthy {
                println!("{}", "Sync is healthy!".green().bold());
            } else {
                println!("{}", "Sync has issues (see above)".yellow().bold());
            }
        }
    }

    Ok(())
}

/// Check if index is up to date (helper function).
fn check_index_up_to_date() -> bool {
    use crate::core::project::{channels_dir, index_path};

    let index_db = index_path();
    let channels = channels_dir();

    // If index doesn't exist, rebuild is needed
    if !index_db.exists() {
        return false;
    }

    // Get index mtime
    let Ok(index_metadata) = std::fs::metadata(&index_db) else {
        return false;
    };
    let Ok(index_mtime) = index_metadata.modified() else {
        return false;
    };

    // Check if channels directory exists
    if !channels.exists() {
        // No channels, no rebuild needed
        return true;
    }

    // Find newest JSONL file
    let mut newest_jsonl_mtime = None;

    if let Ok(entries) = std::fs::read_dir(&channels) {
        for entry in entries.flatten() {
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Ok(metadata) = std::fs::metadata(&path)
                && let Ok(mtime) = metadata.modified()
                && (newest_jsonl_mtime.is_none() || Some(mtime) > newest_jsonl_mtime)
            {
                newest_jsonl_mtime = Some(mtime);
            }
        }
    }

    // If we have JSONL files and the newest is newer than index, rebuild is needed
    if let Some(jsonl_mtime) = newest_jsonl_mtime {
        jsonl_mtime <= index_mtime
    } else {
        // No JSONL files, no rebuild needed
        true
    }
}

/// Pull and push (sync both ways).
pub fn pull_and_push() -> Result<()> {
    pull()?;
    println!();
    push()?;

    Ok(())
}
