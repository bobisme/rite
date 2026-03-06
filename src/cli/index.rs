//! CLI commands for index management.

use anyhow::Result;
use colored::Colorize;

use crate::core::project::{channels_dir, index_path};
use crate::index::IndexSyncer;

/// Rebuild the search index from JSONL files.
pub fn rebuild(if_needed: bool) -> Result<()> {
    if if_needed {
        // Check if rebuild is needed
        let needs_rebuild = check_needs_rebuild()?;

        if !needs_rebuild {
            println!("{} Index is up to date", "Status:".green());
            return Ok(());
        }

        println!(
            "{} JSONL files are newer than index, rebuilding...",
            "Status:".cyan()
        );
    } else {
        println!("{} Rebuilding search index...", "Rebuild:".cyan());
    }

    let mut syncer = IndexSyncer::new()?;
    let stats = syncer.rebuild()?;

    println!("{} Index rebuilt", "Success:".green());
    println!();
    println!("Statistics:");
    println!("  - Channels synced: {}", stats.channels_synced);
    println!("  - Messages indexed: {}", stats.messages_indexed);

    if !stats.errors.is_empty() {
        println!();
        println!("{} Errors encountered:", "Warning:".yellow());
        for err in &stats.errors {
            println!("  - {}", err);
        }
    }

    Ok(())
}

/// Show whether index rebuild is needed.
pub fn status() -> Result<()> {
    let index_db = index_path();

    if !index_db.exists() {
        println!("{} Index does not exist", "Status:".yellow());
        println!("  - Index path: {}", index_db.display());
        println!("  - Recommendation: Run 'rite index rebuild'");
        return Ok(());
    }

    let needs_rebuild = check_needs_rebuild()?;

    if needs_rebuild {
        println!("{} Index rebuild needed", "Status:".yellow());
        println!("  - JSONL files are newer than index");
        println!("  - Recommendation: Run 'rite index rebuild --if-needed'");
    } else {
        println!("{} Index is up to date", "Status:".green());
    }

    // Show index stats
    let syncer = IndexSyncer::new()?;
    let count = syncer.index().message_count()?;
    println!();
    println!("Index statistics:");
    println!("  - Messages indexed: {}", count);
    println!("  - Index path: {}", index_db.display());

    Ok(())
}

/// Check if index rebuild is needed by comparing mtimes.
fn check_needs_rebuild() -> Result<bool> {
    let index_db = index_path();
    let channels = channels_dir();

    // If index doesn't exist, rebuild is needed
    if !index_db.exists() {
        return Ok(true);
    }

    // Get index mtime
    let index_metadata = std::fs::metadata(&index_db)?;
    let index_mtime = index_metadata.modified()?;

    // Check if channels directory exists
    if !channels.exists() {
        // No channels, no rebuild needed
        return Ok(false);
    }

    // Find newest JSONL file
    let mut newest_jsonl_mtime = None;

    for entry in std::fs::read_dir(&channels)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "jsonl") {
            let metadata = std::fs::metadata(&path)?;
            let mtime = metadata.modified()?;

            if newest_jsonl_mtime.is_none() || Some(mtime) > newest_jsonl_mtime {
                newest_jsonl_mtime = Some(mtime);
            }
        }
    }

    // If we have JSONL files and the newest is newer than index, rebuild is needed
    if let Some(jsonl_mtime) = newest_jsonl_mtime {
        Ok(jsonl_mtime > index_mtime)
    } else {
        // No JSONL files, no rebuild needed
        Ok(false)
    }
}
