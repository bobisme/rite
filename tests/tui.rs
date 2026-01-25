//! TUI (Terminal User Interface) tests.
//!
//! Tests for the terminal UI using tmux as a test harness.

mod common;

use common::{TestProject, TuiHarness};

/// Test that TUI starts and shows basic structure.
#[test]
fn test_tui_starts() {
    let mut project = TestProject::with_name("tui-start");

    // Create an agent and send a message to have some content
    let agent = project.agent("TuiTester");
    agent.send("general", "Test message").assert_success();

    let tui = TuiHarness::start(&project);

    // Should be running
    assert!(tui.is_running(), "TUI should be running");

    // Should show basic structure
    let capture = tui.capture();

    // Should have channels/conversations panel (shows either "Channels" or "Conversations" or "general")
    assert!(
        capture.contains("Channels")
            || capture.contains("Conversations")
            || capture.contains("general"),
        "Expected channels panel, got:\n{}",
        capture
    );

    // NOTE: Agents panel was removed in the stateless agent model migration.
    // Agents are now derived from message history rather than having their own panel.

    // Cleanup happens automatically in Drop
}

/// Test that 'q' quits the TUI.
#[test]
fn test_tui_quit_with_q() {
    let project = TestProject::with_name("tui-quit-q");

    let tui = TuiHarness::start(&project);
    assert!(tui.is_running(), "TUI should start running");

    // Send 'q' to quit
    tui.send_keys("q");

    // Should exit within reasonable time
    assert!(
        tui.wait_for_exit(2000),
        "TUI should exit after pressing 'q'"
    );
}

/// Test that Escape quits the TUI.
#[test]
fn test_tui_quit_with_escape() {
    let project = TestProject::with_name("tui-quit-esc");

    let tui = TuiHarness::start(&project);
    assert!(tui.is_running(), "TUI should start running");

    // Send Escape to quit
    tui.send_special("Escape");

    // Should exit within reasonable time
    assert!(
        tui.wait_for_exit(2000),
        "TUI should exit after pressing Escape"
    );
}

/// Test Tab navigation between panes.
#[test]
fn test_tui_tab_navigation() {
    let mut project = TestProject::with_name("tui-nav");

    project.agent("NavTester");

    let tui = TuiHarness::start(&project);

    // Initial state
    let initial = tui.capture();

    // Press Tab to switch panes
    tui.send_special("Tab");

    // The capture should change (border highlight moves)
    // We can't easily detect color, but we verify it doesn't crash
    let after_tab = tui.capture();

    // Both captures should show the basic structure
    assert!(initial.contains("Channels") || initial.contains("general"));
    assert!(after_tab.contains("Channels") || after_tab.contains("general"));

    // Quit
    tui.send_keys("q");
    tui.wait_for_exit(1000);
}

/// Test j/k scrolling in messages.
#[test]
fn test_tui_message_scrolling() {
    let mut project = TestProject::with_name("tui-scroll");

    let agent = project.agent("Scroller");

    // Add many messages so we have something to scroll
    for i in 0..20 {
        agent
            .send("general", &format!("Scroll test message {}", i))
            .assert_success();
    }

    let tui = TuiHarness::start(&project);

    // Capture initial state
    let initial = tui.capture();

    // Press k to scroll up (should show older messages)
    for _ in 0..5 {
        tui.send_keys("k");
    }

    let after_scroll_up = tui.capture();

    // Press j to scroll down
    for _ in 0..3 {
        tui.send_keys("j");
    }

    let after_scroll_down = tui.capture();

    // All captures should contain messages
    assert!(initial.contains("message") || initial.contains("Scroller"));
    assert!(after_scroll_up.contains("message") || after_scroll_up.contains("Scroller"));
    assert!(after_scroll_down.contains("message") || after_scroll_down.contains("Scroller"));

    // Quit
    tui.send_keys("q");
    tui.wait_for_exit(1000);
}

/// Test g/G jump to top/bottom.
#[test]
fn test_tui_jump_keys() {
    let mut project = TestProject::with_name("tui-jump");

    let agent = project.agent("Jumper");

    // Add many messages
    for i in 0..15 {
        agent
            .send("general", &format!("Jump test message {}", i))
            .assert_success();
    }

    let tui = TuiHarness::start(&project);

    // Press 'g' to jump to top (oldest messages)
    tui.send_keys("g");
    let at_top = tui.capture();

    // Press 'G' to jump to bottom (newest messages)
    tui.send_keys("G");
    let at_bottom = tui.capture();

    // Both captures should show messages
    assert!(at_top.contains("message") || at_top.contains("Jumper"));
    assert!(at_bottom.contains("message") || at_bottom.contains("Jumper"));

    // Quit
    tui.send_keys("q");
    tui.wait_for_exit(1000);
}

/// Test that TUI shows messages from multiple agents.
#[test]
fn test_tui_multi_agent_messages() {
    let mut project = TestProject::with_name("tui-multi");

    let alice = project.agent("Alice");
    let bob = project.agent("Bob");

    alice.send("general", "Hello from Alice").assert_success();
    bob.send("general", "Hello from Bob").assert_success();
    alice.send("general", "Another from Alice").assert_success();

    let tui = TuiHarness::start(&project);

    let capture = tui.capture();

    // Should show messages from both agents
    assert!(
        capture.contains("Alice"),
        "Expected Alice in TUI:\n{}",
        capture
    );
    assert!(capture.contains("Bob"), "Expected Bob in TUI:\n{}", capture);

    // Quit
    tui.send_keys("q");
    tui.wait_for_exit(1000);
}

/// Test TUI with agent identity set.
#[test]
fn test_tui_with_agent_identity() {
    let mut project = TestProject::with_name("tui-identity");

    project.agent("IdentityAgent");

    // Start TUI with specific agent identity
    let tui = TuiHarness::start_as(&project, "IdentityAgent");

    let capture = tui.capture();

    // Should show the TUI (we can't easily verify the agent is highlighted without color)
    assert!(capture.contains("Channels") || capture.contains("general"));

    // Quit
    tui.send_keys("q");
    tui.wait_for_exit(1000);
}
