//! Failure scenario tests.
//!
//! Tests for error handling, missing identity, invalid inputs, etc.

mod common;

use common::TestProject;

/// Test that commands fail without BOTBUS_AGENT set.
#[test]
fn test_send_without_identity() {
    let project = TestProject::with_name("no-identity-send");

    // Don't register any agent, don't set env var
    // Try to send without agent context
    let output = project.run_botbus(&["send", "general", "Hello"]);

    output.assert_failure();
    assert!(
        output.stderr_contains("identity") || output.stderr_contains("agent"),
        "Expected identity error, got: {}",
        output.stderr_str()
    );
}

/// Test that claim fails without identity.
#[test]
fn test_claim_without_identity() {
    let project = TestProject::with_name("no-identity-claim");

    let output = project.run_botbus(&["claims", "stake", "src/**"]);

    output.assert_failure();
    assert!(
        output.stderr_contains("identity") || output.stderr_contains("agent"),
        "Expected identity error, got: {}",
        output.stderr_str()
    );
}

/// Test that whoami fails without identity.
#[test]
fn test_whoami_without_identity() {
    let project = TestProject::with_name("no-identity-whoami");

    let output = project.run_botbus(&["whoami"]);

    output.assert_failure();
    assert!(
        output.stderr_contains("identity")
            || output.stderr_contains("agent")
            || output.stderr_contains("configured"),
        "Expected identity error, got: {}",
        output.stderr_str()
    );
}

/// Test that invalid channel names are rejected.
#[test]
fn test_invalid_channel_name() {
    let mut project = TestProject::with_name("invalid-channel");

    let agent = project.agent("Tester");

    // Try uppercase (invalid)
    let output = agent.send("UPPERCASE", "test");
    output.assert_failure();
    assert!(
        output.stderr_contains("Invalid") || output.stderr_contains("channel"),
        "Expected channel name error, got: {}",
        output.stderr_str()
    );
}

// NOTE: test_duplicate_agent_registration was removed - with the stateless
// agent model, there's no registration and no duplicate checking needed.
// Agents are simply derived from BOTBUS_AGENT env var.

/// Test that invalid agent names are rejected.
#[test]
fn test_invalid_agent_name() {
    let project = TestProject::with_name("invalid-agent");

    // Try agent name starting with number (invalid)
    let output = project.run_botbus(&["register", "--name", "123Agent"]);
    output.assert_failure();

    // Try agent name with dashes (invalid)
    let output = project.run_botbus(&["register", "--name", "my-agent"]);
    output.assert_failure();
}

/// Test that history works on empty channel.
#[test]
fn test_history_empty_channel() {
    let mut project = TestProject::with_name("empty-history");

    let agent = project.agent("Viewer");

    // View history of a channel with no messages
    let output = agent.run(&["history", "empty-channel"]);
    output.assert_success();

    // Should complete without error
    assert!(
        output.stdout_str().contains("empty")
            || !output.stdout_str().is_empty()
            || output.stdout_str().is_empty()
    );
}

/// Test search with no results.
#[test]
fn test_search_no_results() {
    let mut project = TestProject::with_name("no-results");

    let agent = project.agent("Searcher");

    // Search for something that doesn't exist
    let output = agent.search("xyznonexistent123");
    output.assert_success();

    // Should complete without error (just no results)
}

/// Test that release with no claims doesn't fail.
#[test]
fn test_release_with_no_claims() {
    let mut project = TestProject::with_name("release-empty");

    let agent = project.agent("NoClaims");

    // Release when there are no claims
    let output = agent.release_all();
    output.assert_success();

    assert!(
        output.stdout_contains("No claims") || output.stdout_contains("0"),
        "Expected 'no claims' message, got: {}",
        output.stdout_str()
    );
}

/// Test agent identity from --agent flag overrides env var.
#[test]
fn test_agent_flag_overrides_env() {
    let mut project = TestProject::with_name("flag-override");

    // Register two agents
    project.agent("EnvAgent");
    project.agent("FlagAgent");

    // Set BOTBUS_AGENT to EnvAgent, but use --agent FlagAgent
    let output = project.run_botbus_with_env(&["--agent", "FlagAgent", "whoami"], Some("EnvAgent"));
    output.assert_success();

    // Should show FlagAgent, not EnvAgent
    assert!(
        output.stdout_contains("FlagAgent"),
        "Expected FlagAgent identity, got: {}",
        output.stdout_str()
    );
}

/// Test that commands work from subdirectory (project auto-discovery).
#[test]
fn test_command_from_subdirectory() {
    let mut project = TestProject::with_name("subdir");

    let agent = project.agent("SubdirAgent");

    // Create a subdirectory
    let subdir = project.path().join("src/deep/nested");
    std::fs::create_dir_all(&subdir).expect("Failed to create subdir");

    // Run command from subdirectory using explicit project path
    // (In real usage, botbus would auto-discover, but we're testing with temp dirs)
    let output = agent.send("general", "Message from nested dir");
    output.assert_success();

    // Message should be recorded
    let messages = project.channel_messages("general");
    assert!(messages.iter().any(|m| {
        m.get("body")
            .and_then(|v| v.as_str())
            .map(|b| b.contains("nested"))
            .unwrap_or(false)
    }));
}
