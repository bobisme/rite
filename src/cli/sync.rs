//! CLI commands for git sync.

use anyhow::Result;
use colored::Colorize;

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
                Err(e) => {
                    eprintln!("{} Failed to rebuild index: {}", "Warning:".yellow(), e);
                    eprintln!("  You can manually rebuild with: bus index rebuild");
                }
            },
            Err(e) => {
                eprintln!("{} Failed to open index: {}", "Warning:".yellow(), e);
                eprintln!("  You can manually rebuild with: bus index rebuild");
            }
        }
    }

    Ok(())
}

/// Show git status.
pub fn status() -> Result<()> {
    let dir = data_dir();

    let status_output = git::status(&dir)?;

    println!("{}", "Git Status:".bold());
    println!();
    print!("{}", status_output);

    Ok(())
}

/// Pull and push (sync both ways).
pub fn pull_and_push() -> Result<()> {
    pull()?;
    println!();
    push()?;

    Ok(())
}
