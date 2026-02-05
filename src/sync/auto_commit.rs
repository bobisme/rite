//! Auto-commit hooks for BotBus operations.
//!
//! These functions are called after operations to auto-commit changes to git.
//! If git is not initialized, they silently do nothing.

use crate::sync::git;
use std::path::Path;

/// Auto-commit after sending a message to a channel.
pub fn auto_commit_after_send(data_dir: &Path, channel: &str) {
    let file = format!("channels/{}.jsonl", channel);
    let message = format!("add message to #{}", channel);

    // Best-effort commit - log warning on error but don't fail
    if let Err(e) = git::commit_files(data_dir, &[&file], &message) {
        eprintln!("Warning: auto-commit failed: {}", e);
    }
}

/// Auto-commit after claiming files/resources.
pub fn auto_commit_after_claim(data_dir: &Path, patterns: &[String]) {
    let message = format!("claim {}", patterns.join(", "));

    if let Err(e) = git::commit_files(data_dir, &["claims.jsonl"], &message) {
        eprintln!("Warning: auto-commit failed: {}", e);
    }
}

/// Auto-commit after releasing claims.
pub fn auto_commit_after_release(data_dir: &Path, claim_id: &str) {
    let message = format!("release claim {}", claim_id);

    if let Err(e) = git::commit_files(data_dir, &["claims.jsonl"], &message) {
        eprintln!("Warning: auto-commit failed: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_auto_commit_noop_if_not_git_repo() {
        let temp = TempDir::new().unwrap();

        // Should not panic or error
        auto_commit_after_send(temp.path(), "general");
        auto_commit_after_claim(temp.path(), &["src/**".to_string()]);
        auto_commit_after_release(temp.path(), "claim-123");
    }
}
