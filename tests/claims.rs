//! File claim conflict tests.
//!
//! Tests for claim conflicts, overlapping patterns, and claim lifecycle.

mod common;

use common::TestProject;

/// Test that claiming already-claimed files is denied.
#[test]
fn test_claim_conflict_denied() {
    let mut project = TestProject::with_name("claim-conflict");

    let agent1 = project.agent("FirstClaimer");
    let agent2 = project.agent("SecondClaimer");

    // First agent claims a directory
    agent1.claim(&["src/**"]).assert_success();

    // Second agent tries to claim overlapping files - should be denied
    let output = agent2.claim(&["src/main.rs"]);
    output.assert_failure();

    // Verify error message explains the conflict and suggests resolution
    assert!(
        output.stderr_contains("Conflict") || output.stderr_contains("conflict"),
        "Expected conflict error, got: {}",
        output.stderr_str()
    );
    assert!(
        output.stderr_contains("FirstClaimer"),
        "Expected to mention claim owner, got: {}",
        output.stderr_str()
    );
    assert!(
        output.stderr_contains("botbus send @"),
        "Expected to suggest messaging, got: {}",
        output.stderr_str()
    );
}

/// Test that agents can't release other agents' claims.
#[test]
fn test_cannot_release_others_claims() {
    let mut project = TestProject::with_name("release-others");

    let owner = project.agent("Owner");
    let other = project.agent("Other");

    // Owner claims files
    owner.claim(&["important/**"]).assert_success();

    // Other agent tries to release (should not release owner's claim)
    other.release_all().assert_success();

    // Owner's claim should still be active
    let claims = project.active_claims();
    assert_eq!(claims.len(), 1);
    assert_eq!(
        claims[0].get("agent").and_then(|v| v.as_str()),
        Some("Owner")
    );
}

/// Test that releasing specific patterns works correctly.
#[test]
fn test_release_specific_pattern() {
    let mut project = TestProject::with_name("release-specific");

    let agent = project.agent("MultiClaimer");

    // Claim multiple patterns
    agent.claim(&["src/**"]).assert_success();
    agent.claim(&["tests/**"]).assert_success();

    let claims_before = project.active_claims();
    assert_eq!(claims_before.len(), 2);

    // Release just one pattern
    agent.run(&["release", "src/**"]).assert_success();

    // Should have one claim left
    let claims_after = project.active_claims();
    assert_eq!(claims_after.len(), 1);

    // The remaining claim should be for tests/** (stored as absolute path)
    let patterns: Vec<&str> = claims_after[0]
        .get("patterns")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|p| p.as_str()).collect())
        .unwrap_or_default();
    // Patterns are now stored with absolute paths, so check suffix
    assert!(
        patterns.iter().any(|p| p.ends_with("tests/**")),
        "Expected pattern ending with tests/**, got: {:?}",
        patterns
    );
}

/// Test claim with TTL (time-to-live).
#[test]
fn test_claim_with_ttl() {
    let mut project = TestProject::with_name("claim-ttl");

    let agent = project.agent("TtlAgent");

    // Claim with short TTL
    agent
        .run(&["claim", "src/**", "--ttl", "3600"])
        .assert_success();

    let claims = project.active_claims();
    assert_eq!(claims.len(), 1);

    // Check that expires_at is set
    let claim = &claims[0];
    assert!(claim.get("expires_at").is_some());
}

/// Test multiple non-overlapping claims from different agents.
#[test]
fn test_multiple_non_overlapping_claims() {
    let mut project = TestProject::with_name("non-overlapping");

    let frontend = project.agent("Frontend");
    let backend = project.agent("Backend");
    let infra = project.agent("Infra");

    // Each agent claims non-overlapping paths
    frontend.claim(&["src/frontend/**"]).assert_success();
    backend.claim(&["src/backend/**"]).assert_success();
    infra.claim(&["infra/**", "docker/**"]).assert_success();

    // Should have 3 separate claims
    let claims = project.active_claims();
    assert_eq!(claims.len(), 3);
}

/// Test claim notification message is posted to general.
#[test]
fn test_claim_posts_message() {
    let mut project = TestProject::with_name("claim-message");

    let agent = project.agent("Announcer");

    // Clear existing messages by noting the count
    let before = project.channel_messages("claims").len();

    // Make a claim with a message
    agent
        .claim_with_message(&["config/**"], "Updating configuration")
        .assert_success();

    // Should have posted a message to #claims
    let messages = project.channel_messages("claims");
    assert!(messages.len() > before);

    // Last message should mention the claim
    let last_msg = messages.last().unwrap();
    let body = last_msg.get("body").and_then(|v| v.as_str()).unwrap_or("");
    assert!(body.contains("config") || body.contains("Claimed"));
}

/// Test release notification message is posted to #claims.
#[test]
fn test_release_posts_message() {
    let mut project = TestProject::with_name("release-message");

    let agent = project.agent("Releaser");

    agent.claim(&["files/**"]).assert_success();

    let before = project.channel_messages("claims").len();

    agent.release_all().assert_success();

    let messages = project.channel_messages("claims");
    assert!(messages.len() > before);

    // Last message should mention release
    let last_msg = messages.last().unwrap();
    let body = last_msg.get("body").and_then(|v| v.as_str()).unwrap_or("");
    assert!(body.contains("Release") || body.contains("files"));
}

/// Test claim listing shows correct information.
#[test]
fn test_claims_listing() {
    let mut project = TestProject::with_name("claims-list");

    let agent1 = project.agent("Agent1");
    let agent2 = project.agent("Agent2");

    agent1.claim(&["src/**"]).assert_success();
    agent2.claim(&["tests/**"]).assert_success();

    // List claims
    let output = agent1.run(&["claims"]);
    output.assert_success();

    // Should show both claims
    let stdout = output.stdout_str();
    assert!(stdout.contains("Agent1") || stdout.contains("src"));
    assert!(stdout.contains("Agent2") || stdout.contains("tests"));
}
