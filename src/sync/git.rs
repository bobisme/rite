//! Git operations for Rite data directory sync.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;
use tracing::{debug, warn};

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
         local/\n\
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
            "chore: initialize rite data repo",
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
                "chore: add existing rite data",
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
            warn!("failed to push to remote during init; run 'rite sync --push' manually");
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

    // Git's own output must never reach the caller's stdout: auto-commit runs
    // inside `send` and `claims stake`, whose stdout is a structured envelope
    // under `--format json|toon`. Capture everything and report through
    // tracing instead.

    // Add files
    let mut cmd = Command::new("git");
    cmd.current_dir(data_dir).arg("add");
    for file in files {
        cmd.arg(file);
    }

    match cmd.output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            warn!(
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "git add failed (auto-commit)"
            );
            return Ok(());
        }
        Err(error) => {
            warn!(%error, "git add failed (auto-commit)");
            return Ok(());
        }
    }

    // Commit (disable GPG signing to avoid interactive prompts)
    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["-c", "commit.gpgsign=false", "commit", "-m", message])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            debug!(
                summary = %String::from_utf8_lossy(&output.stdout).trim(),
                "auto-commit"
            );
        }
        Ok(output) => {
            // Expected when the file has not changed: nothing to commit.
            debug!(
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "auto-commit skipped"
            );
        }
        Err(error) => {
            warn!(%error, "git commit failed (auto-commit)");
        }
    }

    Ok(())
}

/// Commit all uncommitted changes in the data directory.
///
/// Returns true if a commit was made, false if there was nothing to commit.
pub fn commit_all(data_dir: &Path, message: &str) -> Result<bool> {
    if !check_git_available() {
        bail!("git is not installed or not in PATH. Please install git to use sync features.");
    }

    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'rite sync init' first.");
    }

    // Stage everything
    let status = Command::new("git")
        .current_dir(data_dir)
        .args(["add", "-A"])
        .status()
        .context("Failed to run git add")?;

    if !status.success() {
        bail!("git add failed");
    }

    // Host-local state never syncs. `sync init` ignores `local/`, but a store
    // initialised before that rule existed has no such line, and a store that
    // already committed the directory stays tracked regardless of .gitignore.
    // Dropping it from the index here covers both: a no-op when untracked,
    // and a staged removal when it was, including case variants of the
    // directory name that alias it on case-insensitive filesystems.
    let staged = Command::new("git")
        .current_dir(data_dir)
        .args(["ls-files"])
        .output()
        .context("Failed to list the index")?;
    let tracked = local_paths_in(&staged.stdout);
    if !tracked.is_empty() {
        let mut rm_args: Vec<&str> = vec!["rm", "--cached", "--ignore-unmatch", "--quiet", "--"];
        rm_args.extend(tracked.iter().map(String::as_str));
        let output = Command::new("git")
            .current_dir(data_dir)
            .args(&rm_args)
            .output()
            .context("Failed to run git rm --cached")?;
        if !output.status.success() {
            bail!(
                "git rm --cached local failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    // Commit (disable GPG signing)
    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["-c", "commit.gpgsign=false", "commit", "-m", message])
        .output()
        .context("Failed to run git commit")?;

    if !output.status.success() {
        // Git reports "nothing to commit" on stdout, not stderr.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stdout.contains("nothing to commit") || stderr.contains("nothing to commit") {
            return Ok(false);
        }
        bail!(
            "git commit failed: {}",
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            }
        );
    }

    Ok(true)
}

/// Push local commits to remote.
pub fn push(data_dir: &Path) -> Result<()> {
    if !check_git_available() {
        bail!("git is not installed or not in PATH. Please install git to use sync features.");
    }

    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'rite sync init' first.");
    }

    let tracked = tracked_local_paths(data_dir, "HEAD");
    if !tracked.is_empty() {
        bail!(
            "refusing to push: host-local state is tracked ({}). Run 'rite sync commit' to untrack it, then push again.",
            tracked.join(", ")
        );
    }
    // A clean tip is not enough: every commit in the outgoing range travels,
    // and a commit that added local/** and a later one that deleted it would
    // still ship the blobs.
    let history = local_paths_in_outgoing(data_dir)
        .context("could not inspect the outgoing history for host-local state; refusing to push")?;
    if !history.is_empty() {
        bail!(
            "refusing to push: commits not yet on the remote touch host-local state ({}). Rewrite them out of history (for example with git filter-repo --path local --invert-paths) before pushing.",
            history.join(", ")
        );
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
            bail!("Push rejected. Try pulling first with 'rite sync pull'.");
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
        bail!("Not a git repository. Run 'rite sync init' first.");
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

    // Pin what was fetched to one immutable commit, check that commit, and
    // merge that commit by id. Checking and merging a ref by name would let
    // a concurrent fetch swap the ref between the two.
    let resolved = Command::new("git")
        .current_dir(data_dir)
        .args(["rev-parse", "--verify", "origin/main^{commit}"])
        .output()
        .context("Failed to resolve origin/main")?;
    if !resolved.status.success() {
        bail!(
            "could not resolve origin/main: {}",
            String::from_utf8_lossy(&resolved.stderr).trim()
        );
    }
    let remote_oid = String::from_utf8_lossy(&resolved.stdout).trim().to_string();

    // Never merge a remote that tracks host-local state: its session records
    // and adapter table would land in this working tree, and a concurrent
    // send could act on them. The remote history has to be cleaned first.
    let remote_local = tracked_local_paths(data_dir, &remote_oid);
    if !remote_local.is_empty() {
        bail!(
            "refusing to merge {}: the remote tracks host-local state ({}). Nothing was changed. Clean it out of the remote history before pulling again.",
            &remote_oid[..12.min(remote_oid.len())],
            remote_local.join(", ")
        );
    }

    // Merge with union strategy (configured in .gitattributes)
    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["merge", &remote_oid])
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

    // Whatever the remote had under local/ is not this host's state.
    let quarantined = quarantine_imported_local(data_dir)?;
    if !quarantined.is_empty() {
        eprintln!(
            "warning: sync pull brought in host-local state ({}); moved it to local/quarantine/ and untracked it. It was not applied.",
            quarantined.join(", ")
        );
    }

    Ok(changed)
}

/// Whether a repository path is host-local state: its first component is
/// `local`, compared case-insensitively. On a case-insensitive filesystem
/// (macOS by default) `LOCAL/sessions.jsonl` is the same file as
/// `local/sessions.jsonl`, so an exact-path check would let a case-variant
/// alias the runtime paths. Every guard below lists whole trees and filters
/// with this instead of asking git for the exact pathspec.
pub fn is_local_state_path(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    first.eq_ignore_ascii_case("local")
}

fn local_paths_in(listing: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(listing)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && is_local_state_path(l))
        .collect()
}

/// Host-local paths (`local/**`) that are tracked in `tree` (`HEAD`,
/// `origin/main`, ...). Session records and adapter tables under `local/`
/// name things that only mean something on the host that wrote them; a
/// tracked copy is either a leftover from a store initialised before the
/// ignore rule or something imported from a remote, and neither may be
/// acted on.
pub fn tracked_local_paths(data_dir: &Path, tree: &str) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(data_dir)
        .args(["ls-tree", "-r", "--name-only", tree])
        .output();
    match output {
        Ok(o) if o.status.success() => local_paths_in(&o.stdout),
        _ => Vec::new(),
    }
}

/// `local/**` paths present in the tree of any commit a push would send:
/// every commit reachable from `main` and not from `origin/main` (all of
/// `main` if there is no remote branch yet). Trees are inspected commit by
/// commit, without path-based history simplification, so a side branch that
/// added and deleted local state and merged TREESAME is still caught. `Err`
/// means git could not answer, and the caller must fail closed.
pub fn local_paths_in_outgoing(data_dir: &Path) -> Result<Vec<String>> {
    let has_upstream = Command::new("git")
        .current_dir(data_dir)
        .args(["rev-parse", "--verify", "--quiet", "origin/main"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let range = if has_upstream {
        "origin/main..main"
    } else {
        "main"
    };
    let list = Command::new("git")
        .current_dir(data_dir)
        .args(["rev-list", range])
        .output()
        .context("Failed to run git rev-list")?;
    if !list.status.success() {
        bail!(
            "git rev-list {} failed: {}",
            range,
            String::from_utf8_lossy(&list.stderr).trim()
        );
    }
    let mut paths: Vec<String> = Vec::new();
    for sha in String::from_utf8_lossy(&list.stdout).lines() {
        let sha = sha.trim();
        if sha.is_empty() {
            continue;
        }
        let tree = Command::new("git")
            .current_dir(data_dir)
            .args(["ls-tree", "-r", "--name-only", sha])
            .output()
            .context("Failed to run git ls-tree")?;
        if !tree.status.success() {
            bail!(
                "git ls-tree {} failed: {}",
                sha,
                String::from_utf8_lossy(&tree.stderr).trim()
            );
        }
        paths.extend(local_paths_in(&tree.stdout));
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Whether the working tree's index tracks anything under `local/`.
pub fn local_state_is_tracked(data_dir: &Path) -> bool {
    if !is_git_repo(data_dir) {
        return false;
    }
    // Fail closed: if the index cannot be listed, treat it as tracked.
    match Command::new("git")
        .current_dir(data_dir)
        .args(["ls-files"])
        .output()
    {
        Ok(o) if o.status.success() => !local_paths_in(&o.stdout).is_empty(),
        _ => true,
    }
}

/// After a merge, take any `local/**` the remote brought in out of the
/// index and move the files aside under `local/quarantine/<ts>/`, so this
/// host's own session log and adapter table are never replaced by a remote's
/// and nothing imported is ever executed. Returns the quarantined paths.
fn quarantine_imported_local(data_dir: &Path) -> Result<Vec<String>> {
    let tracked = tracked_local_paths(data_dir, "HEAD");
    if tracked.is_empty() {
        return Ok(Vec::new());
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let quarantine = data_dir.join("local").join("quarantine").join(&stamp);
    std::fs::create_dir_all(&quarantine)
        .with_context(|| format!("Failed to create {}", quarantine.display()))?;
    for rel in &tracked {
        let from = data_dir.join(rel);
        if from.exists() {
            let name = rel
                .split_once('/')
                .map(|x| x.1)
                .unwrap_or(rel)
                .replace('/', "__");
            std::fs::rename(&from, quarantine.join(name))
                .with_context(|| format!("Failed to quarantine {}", from.display()))?;
        }
    }
    let rm = {
        let mut rm_args: Vec<&str> = vec!["rm", "--cached", "--ignore-unmatch", "--quiet", "--"];
        rm_args.extend(tracked.iter().map(String::as_str));
        Command::new("git")
            .current_dir(data_dir)
            .args(&rm_args)
            .output()
            .context("Failed to run git rm --cached")?
    };
    if !rm.status.success() {
        bail!(
            "could not untrack imported local/**: {}",
            String::from_utf8_lossy(&rm.stderr).trim()
        );
    }
    let commit = Command::new("git")
        .current_dir(data_dir)
        .args([
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "chore: quarantine host-local state imported by sync pull",
        ])
        .output()
        .context("Failed to commit quarantine")?;
    if !commit.status.success() {
        let err = String::from_utf8_lossy(&commit.stderr);
        let out = String::from_utf8_lossy(&commit.stdout);
        if !err.contains("nothing to commit") && !out.contains("nothing to commit") {
            bail!("could not commit quarantine: {}", err.trim());
        }
    }
    Ok(tracked)
}

/// Get git status (staged, unstaged, ahead/behind).
pub fn status(data_dir: &Path) -> Result<String> {
    if !is_git_repo(data_dir) {
        bail!("Not a git repository. Run 'rite sync init' first.");
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
        bail!("Not a git repository. Run 'rite sync init' first.");
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
        // This test assumes git is installed (required for Rite development)
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

#[cfg(test)]
mod local_state_tests {
    use super::is_local_state_path;

    #[test]
    fn local_state_paths_are_matched_case_insensitively_on_the_first_component() {
        for p in [
            "local/sessions.jsonl",
            "LOCAL/adapters.json",
            "Local/x/y",
            "/local/z",
        ] {
            assert!(is_local_state_path(p), "{p}");
        }
        for p in [
            "channels/local.jsonl",
            "localhost/x",
            "claims.jsonl",
            "nonlocal/a",
            "",
        ] {
            assert!(!is_local_state_path(p), "{p}");
        }
    }
}
