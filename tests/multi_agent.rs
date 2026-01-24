//! Multi-agent coordination simulation tests.
//!
//! These tests simulate realistic scenarios where multiple AI coding agents
//! coordinate their work using BotBus.

mod common;

use common::{Agent, TestProject};
use std::thread;
use std::time::Duration;

/// Simulate a realistic workflow where two agents coordinate on a feature.
///
/// Scenario: FrontendDev and BackendDev are working on a login feature.
/// They need to coordinate file ownership and communicate about the API.
#[test]
fn test_two_agent_feature_coordination() {
    let mut project = TestProject::with_name("feature-coord");

    // === Setup: Register both agents ===
    let frontend = project.agent("FrontendDev");
    let backend = project.agent("BackendDev");

    // Verify both are registered
    let agents = project.registered_agents();
    assert_eq!(agents.len(), 2);

    // === Phase 1: Agents announce their work ===
    frontend
        .send("general", "Starting work on the login page UI")
        .assert_success();
    backend
        .send("general", "I'll handle the authentication API")
        .assert_success();

    // === Phase 2: Agents claim their files ===
    frontend
        .claim_with_message(
            &["src/components/Login.tsx", "src/styles/login.css"],
            "Building login form",
        )
        .assert_success();

    backend
        .claim_with_message(
            &["src/api/auth.rs", "src/api/middleware.rs"],
            "Auth endpoints",
        )
        .assert_success();

    // Verify claims are recorded
    let claims = project.active_claims();
    assert_eq!(claims.len(), 2, "Expected 2 active claims");

    // === Phase 3: Agents coordinate via DM ===
    frontend
        .send("@BackendDev", "What's the endpoint for login?")
        .assert_success();
    backend
        .send(
            "@FrontendDev",
            "POST /api/auth/login with {email, password}",
        )
        .assert_success();
    frontend
        .send(
            "@BackendDev",
            "Got it, thanks! What about the response format?",
        )
        .assert_success();
    backend
        .send(
            "@FrontendDev",
            "Returns {token, user} on success, {error} on failure",
        )
        .assert_success();

    // === Phase 4: Frontend finishes first ===
    frontend
        .send("general", "Login UI complete, releasing files")
        .assert_success();
    frontend.release_all().assert_success();

    // Verify only backend claims remain
    let claims = project.active_claims();
    assert_eq!(
        claims.len(),
        1,
        "Expected 1 active claim after frontend release"
    );

    let remaining_claim = &claims[0];
    assert_eq!(
        remaining_claim.get("agent").and_then(|v| v.as_str()),
        Some("BackendDev")
    );

    // === Phase 5: Backend finishes ===
    backend
        .send("general", "Auth API done. Endpoints ready for integration.")
        .assert_success();
    backend.release_all().assert_success();

    // All claims released
    let claims = project.active_claims();
    assert_eq!(claims.len(), 0, "Expected no active claims");

    // === Verify message history ===
    let messages = project.channel_messages("general");
    assert!(
        messages.len() >= 6,
        "Expected at least 6 messages in general"
    );

    // Check the join messages and work announcements are there
    let bodies: Vec<&str> = messages
        .iter()
        .filter_map(|m| m.get("body").and_then(|v| v.as_str()))
        .collect();

    assert!(bodies.iter().any(|b| b.contains("FrontendDev has joined")));
    assert!(bodies.iter().any(|b| b.contains("BackendDev has joined")));
    assert!(bodies.iter().any(|b| b.contains("login page UI")));
    assert!(bodies.iter().any(|b| b.contains("authentication API")));
}

/// Simulate three agents working in parallel with coordination.
///
/// Scenario: A team of three agents tackling different parts of a system.
#[test]
fn test_three_agent_parallel_work() {
    let mut project = TestProject::with_name("parallel-work");

    let alice = project.agent("Alice");
    let bob = project.agent("Bob");
    let carol = project.agent("Carol");

    // Each agent claims their domain
    alice.claim(&["src/database/**"]).assert_success();
    bob.claim(&["src/api/**"]).assert_success();
    carol.claim(&["src/frontend/**"]).assert_success();

    // All three work "in parallel" (simulated)
    alice
        .send("general", "Setting up database migrations")
        .assert_success();
    bob.send("general", "Creating REST endpoints")
        .assert_success();
    carol
        .send("general", "Building React components")
        .assert_success();

    // Bob needs database info from Alice
    bob.send("@Alice", "What's the schema for the users table?")
        .assert_success();
    alice
        .send("@Bob", "id: uuid, email: string, created_at: timestamp")
        .assert_success();

    // Carol needs API info from Bob
    carol
        .send("@Bob", "What endpoints are available?")
        .assert_success();
    bob.send("@Carol", "GET /users, POST /users, GET /users/:id")
        .assert_success();

    // Alice finishes first
    alice.send("general", "Database ready").assert_success();
    alice.release_all().assert_success();

    // Bob integrates and finishes
    bob.send("general", "API integrated with database, tests passing")
        .assert_success();
    bob.release_all().assert_success();

    // Carol finishes last
    carol
        .send("general", "Frontend connected to API, feature complete")
        .assert_success();
    carol.release_all().assert_success();

    // Verify all claims released
    assert_eq!(project.active_claims().len(), 0);

    // Verify message flow
    let messages = project.channel_messages("general");
    assert!(messages.len() >= 9); // 3 joins + 6 work messages
}

/// Test agent handoff scenario.
///
/// Scenario: One agent starts work, gets blocked, hands off to another.
#[test]
fn test_agent_handoff() {
    let mut project = TestProject::with_name("handoff");

    let starter = project.agent("Starter");
    let finisher = project.agent("Finisher");

    // Starter begins work
    starter
        .claim_with_message(&["src/feature/**"], "Starting new feature")
        .assert_success();
    starter
        .send("general", "Beginning work on user profiles")
        .assert_success();

    // Starter gets blocked and needs to hand off
    starter
        .send(
            "general",
            "I'm blocked on external API access, need to hand off",
        )
        .assert_success();
    starter
        .send("@Finisher", "Can you take over? Files are in src/feature/")
        .assert_success();

    // Starter releases so Finisher can claim
    starter.release_all().assert_success();

    // Finisher picks up
    finisher
        .send("@Starter", "Sure, I'll take it from here")
        .assert_success();
    finisher
        .claim_with_message(&["src/feature/**"], "Continuing Starter's work")
        .assert_success();

    // Verify handoff
    let claims = project.active_claims();
    assert_eq!(claims.len(), 1);
    assert_eq!(
        claims[0].get("agent").and_then(|v| v.as_str()),
        Some("Finisher")
    );

    // Finisher completes
    finisher
        .send("general", "User profiles feature complete")
        .assert_success();
    finisher.release_all().assert_success();

    assert_eq!(project.active_claims().len(), 0);
}

/// Test concurrent message sending (simulated with threads).
#[test]
fn test_concurrent_message_sending() {
    let mut project = TestProject::with_name("concurrent");

    let agent1 = project.agent("Agent1");
    let agent2 = project.agent("Agent2");

    // Clone for threads
    let a1 = agent1.clone();
    let a2 = agent2.clone();

    // Both agents send messages "simultaneously"
    let handle1 = thread::spawn(move || {
        for i in 0..5 {
            a1.send("general", &format!("Agent1 message {}", i))
                .assert_success();
            thread::sleep(Duration::from_millis(10));
        }
    });

    let handle2 = thread::spawn(move || {
        for i in 0..5 {
            a2.send("general", &format!("Agent2 message {}", i))
                .assert_success();
            thread::sleep(Duration::from_millis(10));
        }
    });

    handle1.join().expect("Agent1 thread panicked");
    handle2.join().expect("Agent2 thread panicked");

    // All messages should be recorded (no data loss)
    let messages = project.channel_messages("general");

    // 2 join messages + 10 work messages
    assert_eq!(
        messages.len(),
        12,
        "Expected 12 messages, got {}",
        messages.len()
    );

    // Verify both agents' messages are present
    let bodies: Vec<&str> = messages
        .iter()
        .filter_map(|m| m.get("body").and_then(|v| v.as_str()))
        .collect();

    for i in 0..5 {
        assert!(bodies
            .iter()
            .any(|b| b.contains(&format!("Agent1 message {}", i))));
        assert!(bodies
            .iter()
            .any(|b| b.contains(&format!("Agent2 message {}", i))));
    }
}

/// Test the search functionality across agent messages.
#[test]
fn test_search_across_agents() {
    let mut project = TestProject::with_name("search");

    let dev = project.agent("Developer");
    let reviewer = project.agent("Reviewer");

    // Create searchable content
    dev.send("general", "Implementing authentication with JWT tokens")
        .assert_success();
    dev.send("general", "Added password hashing using bcrypt")
        .assert_success();
    reviewer
        .send("general", "Code review: authentication looks good")
        .assert_success();
    reviewer
        .send("general", "Suggestion: add rate limiting to login endpoint")
        .assert_success();

    // Search for authentication-related messages
    let output = dev.search("authentication");
    output.assert_success();
    assert!(output.stdout_contains("authentication") || output.stdout_contains("JWT"));

    // Search for review comments
    let output = reviewer.search("review");
    output.assert_success();
}

/// Test DM (direct message) channels work correctly.
#[test]
fn test_dm_conversations() {
    let mut project = TestProject::with_name("dm");

    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    // Alice sends DM to Bob
    alice
        .send("@Bob", "Hey Bob, quick question about the API")
        .assert_success();
    bob.send("@Alice", "Sure, what's up?").assert_success();
    alice
        .send("@Bob", "Should we use REST or GraphQL?")
        .assert_success();
    bob.send("@Alice", "REST for now, we can migrate later")
        .assert_success();

    // Verify DM channel was created with correct naming
    // DM channels are named _dm_<sorted_names>
    let dm_channel = if "Alice" < "Bob" {
        "_dm_Alice_Bob"
    } else {
        "_dm_Bob_Alice"
    };

    let dm_messages = project.channel_messages(dm_channel);
    assert_eq!(dm_messages.len(), 4, "Expected 4 DM messages");

    // Verify message content
    let bodies: Vec<&str> = dm_messages
        .iter()
        .filter_map(|m| m.get("body").and_then(|v| v.as_str()))
        .collect();

    assert!(bodies.iter().any(|b| b.contains("REST or GraphQL")));
    assert!(bodies.iter().any(|b| b.contains("REST for now")));
}

/// Test whoami returns correct identity for each agent.
#[test]
fn test_whoami_per_agent() {
    let mut project = TestProject::with_name("whoami");

    let agent1 = project.agent("FirstAgent");
    let agent2 = project.agent("SecondAgent");

    let output1 = agent1.whoami();
    output1.assert_success();
    output1.assert_stdout_contains("FirstAgent");
    output1.assert_stdout_contains("BOTBUS_AGENT");

    let output2 = agent2.whoami();
    output2.assert_success();
    output2.assert_stdout_contains("SecondAgent");
}
