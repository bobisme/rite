//! Tests for message labels and attachments.

mod common;

use common::TestProject;

/// Test sending messages with labels.
#[test]
fn test_send_with_labels() {
    let mut project = TestProject::with_name("labels-send");
    let agent = project.agent("Labeler");

    // Send message with labels
    agent
        .send_with_labels("general", "Bug fix ready for review", &["bug", "ready"])
        .assert_success();

    // Check message has labels in JSON output
    let output = agent.run(&["history", "general", "--json"]);
    output.assert_success();

    let json: serde_json::Value = serde_json::from_str(&output.stdout_str()).unwrap();
    let messages = json["messages"].as_array().unwrap();

    // Find our message (skip registration message)
    let msg = messages
        .iter()
        .find(|m| m["body"].as_str().unwrap().contains("Bug fix"))
        .unwrap();
    let labels = msg["labels"].as_array().unwrap();

    assert_eq!(labels.len(), 2);
    assert!(labels.iter().any(|l| l.as_str().unwrap() == "bug"));
    assert!(labels.iter().any(|l| l.as_str().unwrap() == "ready"));
}

/// Test filtering history by label.
#[test]
fn test_history_filter_by_label() {
    let mut project = TestProject::with_name("labels-filter");
    let agent = project.agent("FilterAgent");

    // Send messages with different labels
    agent
        .send_with_labels("general", "Bug one", &["bug"])
        .assert_success();
    agent
        .send_with_labels("general", "Feature one", &["feature"])
        .assert_success();
    agent
        .send_with_labels("general", "Bug two", &["bug", "urgent"])
        .assert_success();
    agent.send("general", "No labels").assert_success();

    // Filter by "bug" label
    let output = agent.run(&["history", "general", "-L", "bug", "--json"]);
    output.assert_success();

    let json: serde_json::Value = serde_json::from_str(&output.stdout_str()).unwrap();
    let messages = json["messages"].as_array().unwrap();

    // Should have 2 messages with "bug" label
    assert_eq!(messages.len(), 2);
    assert!(messages[0]["body"].as_str().unwrap().contains("Bug one"));
    assert!(messages[1]["body"].as_str().unwrap().contains("Bug two"));
}

/// Test filtering by multiple labels (OR logic).
#[test]
fn test_history_filter_multiple_labels() {
    let mut project = TestProject::with_name("labels-multi");
    let agent = project.agent("MultiLabel");

    agent
        .send_with_labels("general", "Bug", &["bug"])
        .assert_success();
    agent
        .send_with_labels("general", "Feature", &["feature"])
        .assert_success();
    agent
        .send_with_labels("general", "Docs", &["docs"])
        .assert_success();

    // Filter by "bug" OR "feature"
    let output = agent.run(&["history", "general", "-L", "bug", "-L", "feature", "--json"]);
    output.assert_success();

    let json: serde_json::Value = serde_json::from_str(&output.stdout_str()).unwrap();
    let messages = json["messages"].as_array().unwrap();

    // Should have 2 messages (bug and feature, not docs)
    assert_eq!(messages.len(), 2);
}

/// Test sending with file attachment.
#[test]
fn test_send_with_attachment() {
    let mut project = TestProject::with_name("labels-attach");
    let agent = project.agent("Attacher");

    // Create a test file
    std::fs::write(project.path().join("test.txt"), "test content").unwrap();

    // Send with attachment
    let output = agent.run(&["send", "general", "See attached", "--attach", "test.txt"]);
    output.assert_success();

    // Verify attachment in JSON
    let output = agent.run(&["history", "general", "--json"]);
    output.assert_success();

    let json: serde_json::Value = serde_json::from_str(&output.stdout_str()).unwrap();
    let messages = json["messages"].as_array().unwrap();
    let msg = messages
        .iter()
        .find(|m| m["body"].as_str().unwrap().contains("See attached"))
        .unwrap();

    let attachments = msg["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["name"].as_str().unwrap(), "test.txt");
}

/// Test that labels are displayed in non-JSON output.
#[test]
fn test_labels_in_text_output() {
    let mut project = TestProject::with_name("labels-text");
    let agent = project.agent("TextAgent");

    agent
        .send_with_labels("general", "Important update", &["urgent"])
        .assert_success();

    let output = agent.history("general");
    output.assert_success();

    // Should show the label in output
    assert!(output.stdout_str().contains("[urgent]"));
}
