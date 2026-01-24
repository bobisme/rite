//! Integration tests for message read tracking.
//!
//! Note: Registering an agent sends a "joined the project" message to #general.
//! Tests account for these registration messages.

mod common;
use common::TestProject;

#[test]
fn test_inbox_shows_registration_message() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");

    // Registration sends a "joined" message, so inbox should show it
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("unread");
    output.assert_stdout_contains("joined");
}

#[test]
fn test_inbox_shows_new_messages() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");

    // Alice marks general as read to clear registration message
    alice.mark_read("general").assert_success();

    let bob = project.agent("Bob");

    // Bob sends a message
    bob.send("general", "Hello Alice!").assert_success();

    // Alice's inbox should show Bob's registration + his message
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("unread");
    output.assert_stdout_contains("Hello Alice!");
}

#[test]
fn test_inbox_mark_read_clears_unread() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    // Bob sends a message
    bob.send("general", "Hello Alice!").assert_success();

    // Alice marks as read
    alice.inbox_mark_read("general").assert_success();

    // Alice's inbox should now be empty
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("No unread messages");
}

#[test]
fn test_mark_read_command() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    // Bob sends a message
    bob.send("general", "First message").assert_success();

    // Alice marks as read explicitly
    let output = alice.mark_read("general");
    output.assert_success();
    output.assert_stdout_contains("marked #general as read");

    // Alice's inbox should be empty
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("No unread messages");

    // Bob sends another message
    bob.send("general", "Second message").assert_success();

    // Alice should see only the new message
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("1 unread message");
    output.assert_stdout_contains("Second message");
}

#[test]
fn test_per_agent_read_tracking_isolation() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");
    let bob = project.agent("Bob");
    let carol = project.agent("Carol");

    // Carol sends a message
    carol.send("general", "Hello everyone!").assert_success();

    // Alice marks as read (clears all messages including registration)
    alice.mark_read("general").assert_success();

    // Alice's inbox should be empty
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("No unread messages");

    // Bob's inbox should still show messages (he didn't mark read)
    // This includes registration messages and Carol's message
    let output = bob.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("unread");
    output.assert_stdout_contains("Hello everyone!");
}

#[test]
fn test_history_show_offset() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");

    // Send some messages
    alice.send("general", "Message 1").assert_success();
    alice.send("general", "Message 2").assert_success();

    // Get history with offset info
    let output = alice.history_with_offset("general");
    output.assert_success();
    output.assert_stdout_contains("next_offset");
    output.assert_stdout_contains("last_id");
}

#[test]
fn test_history_after_offset() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    // Send first message and get its offset
    alice.send("general", "First message").assert_success();

    // Get the current offset
    let output = alice.history_with_offset("general");
    output.assert_success();
    let stdout = output.stdout_str();

    // Parse offset from output (e.g., "next_offset: 123")
    let offset: u64 = stdout
        .lines()
        .find(|l| l.contains("next_offset:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|s| s.trim().parse().ok())
        .expect("Could not parse offset");

    // Send second message
    bob.send("general", "Second message").assert_success();

    // Read only messages after the offset
    let output = alice.history_after_offset("general", offset);
    output.assert_success();

    // Should only contain the second message
    let stdout = output.stdout_str();
    assert!(
        !stdout.contains("First message"),
        "Should not contain first message"
    );
    assert!(
        stdout.contains("Second message"),
        "Should contain second message"
    );
}

#[test]
fn test_mark_read_at_explicit_offset() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    // Send two messages
    bob.send("general", "Message 1").assert_success();
    bob.send("general", "Message 2").assert_success();

    // Use mark_read_at with offset 0 to reset read position
    let output = alice.mark_read_at("general", 0);
    output.assert_success();

    // Alice should see all messages as unread (registration + bob's 2 messages)
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("unread");
    output.assert_stdout_contains("Message 1");
    output.assert_stdout_contains("Message 2");
}

#[test]
fn test_inbox_multiple_channels() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    // Bob sends to different channels
    bob.send("general", "General message").assert_success();
    bob.send("backend", "Backend message").assert_success();

    // Alice marks general as read
    alice.mark_read("general").assert_success();

    // General should be empty
    let output = alice.inbox("general");
    output.assert_success();
    output.assert_stdout_contains("No unread messages");

    // Backend should still have unread (only Bob's message, no registration there)
    let output = alice.inbox("backend");
    output.assert_success();
    output.assert_stdout_contains("unread");
    output.assert_stdout_contains("Backend message");
}

#[test]
fn test_inbox_count_limit() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");

    // Mark as read to clear registration messages
    alice.mark_read("general").assert_success();

    let bob = project.agent("Bob");

    // Send many messages (10 + 1 registration for bob = 11 new messages)
    for i in 1..=10 {
        bob.send("general", &format!("Msg{}", i)).assert_success();
    }

    // Inbox with limit of 3
    let output = alice.run(&["inbox", "general", "-n", "3"]);
    output.assert_success();

    // Should show 3 messages
    let stdout = output.stdout_str();
    output.assert_stdout_contains("3 unread");
}

#[test]
fn test_mark_read_nonexistent_channel_fails() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");

    // Try to mark read on a channel that doesn't exist
    let output = alice.mark_read("nonexistent");
    output.assert_failure();
    output.assert_stderr_contains("does not exist");
}

#[test]
fn test_inbox_nonexistent_channel_ok() {
    let mut project = TestProject::new();
    let alice = project.agent("Alice");

    // Inbox on nonexistent channel should succeed with "no messages"
    let output = alice.inbox("nonexistent");
    output.assert_success();
    output.assert_stdout_contains("No unread messages");
}
