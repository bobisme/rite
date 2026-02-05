//! Integration tests for git sync functionality.
//!
//! These tests verify that git sync works in complete isolation using temp directories.
//! They test:
//! - `bus sync init` (initialization)
//! - Auto-commit after operations
//! - Two-machine sync simulation
//! - Index rebuild after sync
//! - Sync status/check/log commands

mod common;

use common::TestProject;
use std::path::Path;
use std::process::Command;

/// Helper to run git commands in a directory.
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("Failed to run git command")
}

/// Helper to check if git command succeeded.
fn git_success(dir: &Path, args: &[&str]) -> bool {
    git(dir, args).status.success()
}

/// Helper to get git log count.
fn git_log_count(dir: &Path) -> usize {
    let output = git(dir, &["log", "--oneline"]);
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout).lines().count()
}

/// Helper to disable GPG signing in a git repo (for tests).
fn disable_gpg_signing(dir: &Path) {
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// Test that `bus sync init` creates .git/, .gitattributes, .gitignore.
#[test]
fn test_sync_init_creates_git_infrastructure() {
    let mut project = TestProject::with_name("sync-init");
    let agent = project.agent("TestAgent");
    let data_path = project.data_path();

    // Verify no git repo exists yet
    assert!(!data_path.join(".git").exists());

    // Run sync init
    let output = agent.run(&["sync", "init"]);
    output.assert_success();
    disable_gpg_signing(data_path);

    // Verify .git directory was created
    assert!(
        data_path.join(".git").exists(),
        ".git directory should be created"
    );

    // Verify .gitattributes was created and contains union merge
    let gitattributes_path = data_path.join(".gitattributes");
    assert!(gitattributes_path.exists(), ".gitattributes should exist");

    let gitattributes_content =
        std::fs::read_to_string(&gitattributes_path).expect("Failed to read .gitattributes");
    assert!(
        gitattributes_content.contains("*.jsonl merge=union"),
        ".gitattributes should configure union merge for JSONL files"
    );

    // Verify .gitignore was created and excludes db files
    let gitignore_path = data_path.join(".gitignore");
    assert!(gitignore_path.exists(), ".gitignore should exist");

    let gitignore_content =
        std::fs::read_to_string(&gitignore_path).expect("Failed to read .gitignore");
    assert!(
        gitignore_content.contains("*.db"),
        ".gitignore should exclude *.db"
    );
    assert!(
        gitignore_content.contains("state.json"),
        ".gitignore should exclude state.json"
    );
    assert!(
        gitignore_content.contains("attachments/"),
        ".gitignore should exclude attachments/"
    );

    // Verify initial commit was created
    let log_count = git_log_count(data_path);
    assert!(log_count >= 1, "Should have at least one commit after init");
}

/// Test that running `bus sync init` twice is safe (idempotent or graceful error).
#[test]
fn test_sync_init_idempotent() {
    let mut project = TestProject::with_name("sync-init-twice");
    let agent = project.agent("TestAgent");

    // First init should succeed
    let output1 = agent.run(&["sync", "init"]);
    output1.assert_success();

    // Second init should fail gracefully with clear error
    let output2 = agent.run(&["sync", "init"]);
    output2.assert_failure();
    assert!(
        output2.stderr_contains("already exists"),
        "Should indicate git repo already exists, got: {}",
        output2.stderr_str()
    );
}

/// Test that operations auto-commit after sending messages.
#[test]
fn test_auto_commit_after_send() {
    let mut project = TestProject::with_name("auto-commit-send");
    let agent = project.agent("MessageSender");
    let data_path = project.data_path();

    // Init sync first
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(data_path);

    let commits_before = git_log_count(data_path);

    // Send a message - should trigger auto-commit
    agent.send("general", "Test message").assert_success();

    let commits_after = git_log_count(data_path);
    assert!(
        commits_after > commits_before,
        "Should have new commit after send (before: {}, after: {})",
        commits_before,
        commits_after
    );

    // Verify commit message mentions the channel
    let output = git(data_path, &["log", "-1", "--pretty=%s"]);
    let commit_msg = String::from_utf8_lossy(&output.stdout);
    assert!(
        commit_msg.contains("general") || commit_msg.contains("message"),
        "Commit message should mention channel or message, got: {}",
        commit_msg
    );
}

/// Test that claiming files auto-commits.
#[test]
fn test_auto_commit_after_claim() {
    let mut project = TestProject::with_name("auto-commit-claim");
    let agent = project.agent("FileClaimer");
    let data_path = project.data_path();

    // Init sync first
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(data_path);

    let commits_before = git_log_count(data_path);

    // Claim files - should trigger auto-commit
    agent.run(&["claims", "stake", "src/**"]).assert_success();

    let commits_after = git_log_count(data_path);
    assert!(
        commits_after > commits_before,
        "Should have new commit after claim (before: {}, after: {})",
        commits_before,
        commits_after
    );

    // Verify commit message mentions claim
    let output = git(data_path, &["log", "-1", "--pretty=%s"]);
    let commit_msg = String::from_utf8_lossy(&output.stdout);
    assert!(
        commit_msg.contains("claim") || commit_msg.contains("src"),
        "Commit message should mention claim or pattern, got: {}",
        commit_msg
    );
}

/// Test that releasing claims auto-commits.
#[test]
fn test_auto_commit_after_release() {
    let mut project = TestProject::with_name("auto-commit-release");
    let agent = project.agent("FileReleaser");
    let data_path = project.data_path();

    // Init sync and claim files first
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(data_path);
    agent.run(&["claims", "stake", "tests/**"]).assert_success();

    let commits_before = git_log_count(data_path);

    // Release - should trigger auto-commit
    agent.run(&["claims", "release", "--all"]).assert_success();

    let commits_after = git_log_count(data_path);
    assert!(
        commits_after > commits_before,
        "Should have new commit after release (before: {}, after: {})",
        commits_before,
        commits_after
    );

    // Verify commit message mentions release
    let output = git(data_path, &["log", "-1", "--pretty=%s"]);
    let commit_msg = String::from_utf8_lossy(&output.stdout);
    assert!(
        commit_msg.contains("release"),
        "Commit message should mention release, got: {}",
        commit_msg
    );
}

/// Test operations succeed WITHOUT sync init (auto-commit silently skipped).
#[test]
fn test_operations_work_without_sync() {
    let mut project = TestProject::with_name("no-sync");
    let agent = project.agent("NoSyncAgent");
    let data_path = project.data_path();

    // Verify no git repo
    assert!(!data_path.join(".git").exists());

    // Send message - should succeed even without git
    agent
        .send("general", "Message without git")
        .assert_success();

    // Claim - should succeed
    agent.run(&["claims", "stake", "files/**"]).assert_success();

    // Release - should succeed
    agent.run(&["claims", "release", "--all"]).assert_success();

    // All operations worked despite no git repo
    assert!(
        !data_path.join(".git").exists(),
        "Should still have no git repo"
    );
}

/// THE CRITICAL TEST: Two-machine sync simulation.
///
/// Simulates:
/// machine_a (TestProject) <-> bare_repo <-> machine_b (TestProject)
///
/// Tests:
/// 1. machine_a sends messages, commits, pushes
/// 2. machine_b pulls, sees messages
/// 3. machine_b sends messages, pushes
/// 4. machine_a pulls, sees all messages from both machines
/// 5. No duplicate messages (union merge + dedupe)
#[test]
fn test_two_machine_sync_simulation() {
    use tempfile::TempDir;

    // Create a bare git repo to act as the "origin"
    let bare_repo_dir =
        TempDir::with_prefix("botbus-bare-").expect("Failed to create bare repo dir");
    let bare_repo_path = bare_repo_dir.path();

    // Initialize bare repo
    let init_output = Command::new("git")
        .current_dir(bare_repo_path)
        .args(["init", "--bare"])
        .output()
        .expect("Failed to init bare repo");
    assert!(init_output.status.success(), "Bare repo init failed");

    // Create machine A
    let mut machine_a = TestProject::with_name("machine-a");
    let agent_a = machine_a.agent("MachineA");
    let data_a = machine_a.data_path().to_path_buf();

    // Init sync on machine A
    agent_a.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(&data_a);

    // Configure remote and push
    assert!(
        git_success(
            &data_a,
            &[
                "remote",
                "add",
                "origin",
                &bare_repo_path.display().to_string()
            ]
        ),
        "Failed to add remote for machine A"
    );
    assert!(
        git_success(&data_a, &["push", "-u", "origin", "main"]),
        "Failed to push from machine A"
    );

    // Machine A sends some messages
    agent_a
        .send("general", "Hello from machine A")
        .assert_success();
    agent_a
        .send("general", "Second message from A")
        .assert_success();

    // Push changes from machine A
    assert!(
        git_success(&data_a, &["push", "origin", "main"]),
        "Failed to push messages from machine A"
    );

    // Create machine B
    let mut machine_b = TestProject::with_name("machine-b");
    let agent_b = machine_b.agent("MachineB");
    let data_b = machine_b.data_path().to_path_buf();

    // Init git on machine B and configure remote
    assert!(
        git_success(&data_b, &["init"]),
        "Failed to init git on machine B"
    );
    disable_gpg_signing(&data_b);
    assert!(
        git_success(
            &data_b,
            &[
                "remote",
                "add",
                "origin",
                &bare_repo_path.display().to_string()
            ]
        ),
        "Failed to add remote for machine B"
    );
    assert!(
        git_success(&data_b, &["pull", "origin", "main"]),
        "Failed to pull to machine B"
    );

    // Machine B should see messages from machine A
    let history_b = agent_b.history("general");
    history_b.assert_success();
    let stdout_b = history_b.stdout_str();
    assert!(
        stdout_b.contains("Hello from machine A"),
        "Machine B should see machine A's first message"
    );
    assert!(
        stdout_b.contains("Second message from A"),
        "Machine B should see machine A's second message"
    );

    // Machine B sends its own messages
    agent_b
        .send("general", "Hello from machine B")
        .assert_success();
    agent_b
        .send("general", "Machine B here too")
        .assert_success();

    // Push from machine B
    assert!(
        git_success(&data_b, &["push", "origin", "main"]),
        "Failed to push messages from machine B"
    );

    // Machine A pulls updates from machine B
    assert!(
        git_success(&data_a, &["pull", "origin", "main"]),
        "Failed to pull to machine A"
    );

    // Machine A should now see ALL messages from both machines
    let history_a = agent_a.history("general");
    history_a.assert_success();
    let stdout_a = history_a.stdout_str();

    assert!(
        stdout_a.contains("Hello from machine A"),
        "Machine A should still see its own first message"
    );
    assert!(
        stdout_a.contains("Second message from A"),
        "Machine A should still see its own second message"
    );
    assert!(
        stdout_a.contains("Hello from machine B"),
        "Machine A should see machine B's first message"
    );
    assert!(
        stdout_a.contains("Machine B here too"),
        "Machine A should see machine B's second message"
    );

    // Verify no duplicate messages by counting lines in the JSONL
    let messages_a = machine_a.channel_messages("general");
    assert_eq!(
        messages_a.len(),
        4,
        "Should have exactly 4 messages (no duplicates)"
    );

    // Machine B should also have all 4 messages
    let messages_b = machine_b.channel_messages("general");
    assert_eq!(
        messages_b.len(),
        4,
        "Machine B should also have exactly 4 messages"
    );
}

/// Test index rebuild after sync.
#[test]
fn test_index_rebuild_after_sync() {
    let mut project = TestProject::with_name("index-rebuild");
    let agent = project.agent("Indexer");

    // Send some messages first
    agent.send("general", "First message").assert_success();
    agent.send("general", "Second message").assert_success();

    // Init sync (this will auto-commit existing messages)
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(project.data_path());

    // Rebuild index
    let rebuild_output = agent.run(&["index", "rebuild"]);
    rebuild_output.assert_success();

    // Check index status
    let status_output = agent.run(&["index", "status"]);
    status_output.assert_success();

    // Search should work after rebuild
    let search_output = agent.search("First");
    search_output.assert_success();
    assert!(
        search_output.stdout_contains("First message"),
        "Search should find indexed message"
    );
}

/// Test sync status command.
#[test]
fn test_sync_status() {
    let mut project = TestProject::with_name("sync-status");
    let agent = project.agent("StatusAgent");

    // Before init, status should indicate not a git repo
    let output = agent.run(&["sync", "status"]);
    output.assert_success();
    assert!(
        output.stdout_contains("Not a git repository") || output.stdout_contains("false"),
        "Status should indicate not a git repo before init"
    );

    // After init, status should succeed
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(project.data_path());

    let output = agent.run(&["sync", "status"]);
    output.assert_success();
    // Should show it's a git repo now
    assert!(
        !output.stdout_contains("Not a git repository"),
        "Status should not say 'not a git repo' after init"
    );
}

/// Test sync log command.
#[test]
fn test_sync_log() {
    let mut project = TestProject::with_name("sync-log");
    let agent = project.agent("LogAgent");

    // Init creates initial commit
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(project.data_path());

    // Send message to create another commit
    agent
        .send("general", "Test message for log")
        .assert_success();

    // Check log
    let output = agent.run(&["sync", "log"]);
    output.assert_success();

    let stdout = output.stdout_str();
    // Should show commits (at least the init commit)
    assert!(
        stdout.contains("initialize") || stdout.contains("message"),
        "Log should show commit messages"
    );
}

/// Test sync check command.
#[test]
fn test_sync_check() {
    let mut project = TestProject::with_name("sync-check");
    let agent = project.agent("CheckAgent");

    // Check before init - should report not a git repo
    let output = agent.run(&["sync", "check"]);
    output.assert_success();
    let stdout = output.stdout_str();
    assert!(
        stdout.contains("not initialized") || stdout.contains("false"),
        "Check should report repo not initialized"
    );

    // Init sync
    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(project.data_path());

    // Check after init - should report healthy
    let output = agent.run(&["sync", "check"]);
    output.assert_success();
    let stdout = output.stdout_str();
    // Should show git is available and repo is initialized
    assert!(
        stdout.contains("Git") || stdout.contains("true"),
        "Check should show git info after init"
    );
}

/// Test that sync works with claims (claim data is synced).
#[test]
fn test_sync_claims_between_machines() {
    use tempfile::TempDir;

    // Create bare repo
    let bare_repo_dir =
        TempDir::with_prefix("botbus-bare-claims-").expect("Failed to create bare repo dir");
    let bare_repo_path = bare_repo_dir.path();

    Command::new("git")
        .current_dir(bare_repo_path)
        .args(["init", "--bare"])
        .output()
        .expect("Failed to init bare repo");

    // Machine A: init, claim, push
    let mut machine_a = TestProject::with_name("machine-a-claims");
    let agent_a = machine_a.agent("AgentA");
    let data_a = machine_a.data_path().to_path_buf();

    agent_a.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(&data_a);
    git_success(
        &data_a,
        &[
            "remote",
            "add",
            "origin",
            &bare_repo_path.display().to_string(),
        ],
    );
    git_success(&data_a, &["push", "-u", "origin", "main"]);

    // Make a claim
    agent_a
        .run(&["claims", "stake", "src/important/**"])
        .assert_success();
    git_success(&data_a, &["push", "origin", "main"]);

    // Machine B: init, pull, check claims
    let mut machine_b = TestProject::with_name("machine-b-claims");
    let agent_b = machine_b.agent("AgentB");
    let data_b = machine_b.data_path().to_path_buf();

    git_success(&data_b, &["init"]);
    disable_gpg_signing(&data_b);
    git_success(
        &data_b,
        &[
            "remote",
            "add",
            "origin",
            &bare_repo_path.display().to_string(),
        ],
    );
    git_success(&data_b, &["pull", "origin", "main"]);

    // Machine B should see the claim
    let claims_b = machine_b.active_claims();
    assert_eq!(
        claims_b.len(),
        1,
        "Machine B should see the claim from machine A"
    );
    assert_eq!(
        claims_b[0].get("agent").and_then(|v| v.as_str()),
        Some("AgentA"),
        "Claim should be from AgentA"
    );
}

/// Test sync with JSON output format.
#[test]
fn test_sync_status_json_format() {
    let mut project = TestProject::with_name("sync-json");
    let agent = project.agent("JsonAgent");

    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(project.data_path());

    let output = agent.run(&["sync", "status", "--format", "json"]);
    output.assert_success();

    // Should be valid JSON
    let stdout = output.stdout_str();
    let result: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(
        result.is_ok(),
        "Output should be valid JSON, got: {}",
        stdout
    );
}

/// Test sync with TOON output format.
#[test]
fn test_sync_status_toon_format() {
    let mut project = TestProject::with_name("sync-toon");
    let agent = project.agent("ToonAgent");

    agent.run(&["sync", "init"]).assert_success();
    disable_gpg_signing(project.data_path());

    let output = agent.run(&["sync", "status", "--format", "toon"]);
    output.assert_success();

    let stdout = output.stdout_str();
    // TOON format uses key-value pairs
    assert!(
        stdout.contains(":") || stdout.contains("is_git_repo"),
        "Should output TOON format with key-value pairs"
    );
}
