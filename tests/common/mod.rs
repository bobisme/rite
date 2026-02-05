//! Common test utilities for integration tests.
//!
//! Provides a harness for spawning botbus subprocesses and simulating
//! multi-agent coordination scenarios.
//!
//! Uses BOTBUS_DATA_DIR env var to isolate each test's data directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

static PROJECT_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Get the path to the botbus binary.
pub fn botbus_bin() -> PathBuf {
    // Try release first, fall back to debug
    let release = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/bus");
    if release.exists() {
        return release;
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/bus")
}

/// A test project with an isolated BotBus data directory.
///
/// Each TestProject gets its own temp directory that is set via BOTBUS_DATA_DIR
/// env var when running botbus commands.
pub struct TestProject {
    pub dir: TempDir,
    pub data_path: PathBuf,
    /// Working directory for commands (simulates being in a project)
    pub work_dir: PathBuf,
    agents: HashMap<String, Agent>,
}

impl TestProject {
    /// Create a new test project with isolated data directory.
    pub fn new() -> Self {
        let count = PROJECT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = TempDir::with_prefix(&format!("botbus-test-{}-", count))
            .expect("Failed to create temp dir");
        let data_path = dir.path().join("data");
        let work_dir = dir.path().join("project");

        // Create directories
        std::fs::create_dir_all(&data_path).expect("Failed to create data dir");
        std::fs::create_dir_all(&work_dir).expect("Failed to create work dir");

        Self {
            dir,
            data_path,
            work_dir,
            agents: HashMap::new(),
        }
    }

    /// Create a new test project with a custom name (for debugging).
    pub fn with_name(name: &str) -> Self {
        let dir =
            TempDir::with_prefix(&format!("botbus-{}-", name)).expect("Failed to create temp dir");
        let data_path = dir.path().join("data");
        let work_dir = dir.path().join("project");

        std::fs::create_dir_all(&data_path).expect("Failed to create data dir");
        std::fs::create_dir_all(&work_dir).expect("Failed to create work dir");

        Self {
            dir,
            data_path,
            work_dir,
            agents: HashMap::new(),
        }
    }

    /// Create an agent handle (no registration needed in stateless model).
    ///
    /// In the new model, agents don't need to register - they just need
    /// BOTBUS_AGENT set when running commands.
    pub fn agent(&mut self, name: &str) -> Agent {
        let agent = Agent {
            name: name.to_string(),
            data_path: self.data_path.clone(),
            work_dir: self.work_dir.clone(),
        };

        self.agents.insert(name.to_string(), agent.clone());
        agent
    }

    /// Run a botbus command without agent context.
    pub fn run_botbus(&self, args: &[&str]) -> BotbusOutput {
        self.run_botbus_with_env(args, None)
    }

    /// Run a botbus command with optional agent environment.
    pub fn run_botbus_with_env(&self, args: &[&str], agent: Option<&str>) -> BotbusOutput {
        let mut cmd = Command::new(botbus_bin());
        cmd.current_dir(&self.work_dir);
        cmd.env("BOTBUS_DATA_DIR", &self.data_path);
        cmd.args(args);

        if let Some(agent_name) = agent {
            cmd.env("BOTBUS_AGENT", agent_name);
        } else {
            // Explicitly remove BOTBUS_AGENT to prevent env var leakage
            cmd.env_remove("BOTBUS_AGENT");
        }

        let output = cmd.output().expect("Failed to execute botbus");
        BotbusOutput::from(output)
    }

    /// Get message history for a channel (as raw JSONL content).
    pub fn channel_messages(&self, channel: &str) -> Vec<serde_json::Value> {
        let path = self
            .data_path
            .join("channels")
            .join(format!("{}.jsonl", channel));
        if !path.exists() {
            return Vec::new();
        }

        let content = std::fs::read_to_string(&path).expect("Failed to read channel");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("Invalid JSON in channel"))
            .collect()
    }

    /// Get all active claims.
    pub fn active_claims(&self) -> Vec<serde_json::Value> {
        let path = self.data_path.join("claims.jsonl");
        if !path.exists() {
            return Vec::new();
        }

        let content = std::fs::read_to_string(&path).expect("Failed to read claims");
        let all_claims: Vec<serde_json::Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("Invalid JSON in claims"))
            .collect();

        // Build active claims map (latest state per claim ID, filter active)
        let mut active: HashMap<String, serde_json::Value> = HashMap::new();
        for claim in all_claims {
            if let Some(id) = claim.get("id").and_then(|v| v.as_str()) {
                active.insert(id.to_string(), claim);
            }
        }

        active
            .into_values()
            .filter(|c| c.get("active").and_then(|v| v.as_bool()).unwrap_or(false))
            .collect()
    }

    /// Get list of agents who have sent messages (derived from message history).
    ///
    /// In the stateless model, there's no agents.jsonl - agents are identified
    /// by their presence in message history.
    pub fn registered_agents(&self) -> Vec<String> {
        let channels_dir = self.data_path.join("channels");
        if !channels_dir.exists() {
            return Vec::new();
        }

        let mut agents: std::collections::HashSet<String> = std::collections::HashSet::new();

        if let Ok(entries) = std::fs::read_dir(&channels_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "jsonl") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        for line in content.lines() {
                            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
                                if let Some(agent) = msg.get("agent").and_then(|v| v.as_str()) {
                                    agents.insert(agent.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        agents.into_iter().collect()
    }

    /// Get path to the data directory.
    pub fn data_path(&self) -> &Path {
        &self.data_path
    }

    /// Get path to the work directory (simulated project root).
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// Legacy alias for backwards compatibility.
    pub fn path(&self) -> &Path {
        &self.work_dir
    }
}

/// An agent handle for sending commands as a specific agent.
#[derive(Clone)]
pub struct Agent {
    pub name: String,
    data_path: PathBuf,
    work_dir: PathBuf,
}

impl Agent {
    /// Send a message to a channel.
    pub fn send(&self, target: &str, message: &str) -> BotbusOutput {
        self.run(&["send", target, message])
    }

    /// Send a message with labels.
    pub fn send_with_labels(&self, target: &str, message: &str, labels: &[&str]) -> BotbusOutput {
        let mut args = vec!["send", target, message];
        for label in labels {
            args.push("-L");
            args.push(label);
        }
        self.run(&args)
    }

    /// Get history filtered by label.
    pub fn history_with_label(&self, channel: &str, labels: &[&str]) -> BotbusOutput {
        let mut args = vec!["history", channel];
        for label in labels {
            args.push("-L");
            args.push(label);
        }
        self.run(&args)
    }

    /// Claim file patterns.
    pub fn claim(&self, patterns: &[&str]) -> BotbusOutput {
        let mut args = vec!["claim"];
        args.extend(patterns);
        self.run(&args)
    }

    /// Claim with a message.
    pub fn claim_with_message(&self, patterns: &[&str], message: &str) -> BotbusOutput {
        let mut args = vec!["claim"];
        args.extend(patterns);
        args.extend(&["-m", message]);
        self.run(&args)
    }

    /// Release all claims.
    pub fn release_all(&self) -> BotbusOutput {
        self.run(&["release", "--all"])
    }

    /// Release specific patterns.
    pub fn release(&self, patterns: &[&str]) -> BotbusOutput {
        let mut args = vec!["release"];
        args.extend(patterns);
        self.run(&args)
    }

    /// List claims.
    pub fn claims(&self) -> BotbusOutput {
        self.run(&["claims"])
    }

    /// Get message history for a channel.
    pub fn history(&self, channel: &str) -> BotbusOutput {
        self.run(&["history", channel])
    }

    /// Get message history with count.
    pub fn history_n(&self, channel: &str, count: usize) -> BotbusOutput {
        self.run(&["history", channel, "-n", &count.to_string()])
    }

    /// Get whoami output.
    pub fn whoami(&self) -> BotbusOutput {
        self.run(&["whoami"])
    }

    /// List agents (derived from message history).
    pub fn agents(&self) -> BotbusOutput {
        self.run(&["agents"])
    }

    /// Search messages.
    pub fn search(&self, query: &str) -> BotbusOutput {
        self.run(&["search", query])
    }

    /// Get inbox (unread messages).
    pub fn inbox(&self, channel: &str) -> BotbusOutput {
        self.run(&["inbox", "-c", channel, "--format", "text"])
    }

    /// Get inbox and mark as read.
    pub fn inbox_mark_read(&self, channel: &str) -> BotbusOutput {
        self.run(&["inbox", "-c", channel, "--mark-read", "--format", "text"])
    }

    /// Mark a channel as read.
    pub fn mark_read(&self, channel: &str) -> BotbusOutput {
        self.run(&["mark-read", channel])
    }

    /// Mark a channel as read at a specific offset.
    pub fn mark_read_at(&self, channel: &str, offset: u64) -> BotbusOutput {
        self.run(&["mark-read", channel, "--offset", &offset.to_string()])
    }

    /// Get history with --show-offset.
    pub fn history_with_offset(&self, channel: &str) -> BotbusOutput {
        self.run(&["history", channel, "--show-offset"])
    }

    /// Get history after an offset.
    pub fn history_after_offset(&self, channel: &str, offset: u64) -> BotbusOutput {
        self.run(&["history", channel, "--after-offset", &offset.to_string()])
    }

    /// Run an arbitrary botbus command as this agent.
    pub fn run(&self, args: &[&str]) -> BotbusOutput {
        let mut cmd = Command::new(botbus_bin());
        cmd.current_dir(&self.work_dir);
        cmd.env("BOTBUS_DATA_DIR", &self.data_path);
        cmd.env("BOTBUS_AGENT", &self.name);
        cmd.args(args);

        let output = cmd.output().expect("Failed to execute botbus");
        BotbusOutput::from(output)
    }
}

/// Wrapper around Command output with helper methods.
pub struct BotbusOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl From<Output> for BotbusOutput {
    fn from(output: Output) -> Self {
        Self {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

impl BotbusOutput {
    pub fn success(&self) -> bool {
        self.status.success()
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).to_string()
    }

    pub fn stdout_contains(&self, needle: &str) -> bool {
        self.stdout_str().contains(needle)
    }

    pub fn stderr_contains(&self, needle: &str) -> bool {
        self.stderr_str().contains(needle)
    }

    /// Assert the command succeeded.
    pub fn assert_success(&self) {
        assert!(
            self.success(),
            "Command failed.\nstdout: {}\nstderr: {}",
            self.stdout_str(),
            self.stderr_str()
        );
    }

    /// Assert the command failed.
    pub fn assert_failure(&self) {
        assert!(
            !self.success(),
            "Command unexpectedly succeeded.\nstdout: {}\nstderr: {}",
            self.stdout_str(),
            self.stderr_str()
        );
    }

    /// Assert stdout contains a string.
    pub fn assert_stdout_contains(&self, needle: &str) {
        assert!(
            self.stdout_contains(needle),
            "Expected stdout to contain '{}', got:\n{}",
            needle,
            self.stdout_str()
        );
    }

    /// Assert stderr contains a string.
    pub fn assert_stderr_contains(&self, needle: &str) {
        assert!(
            self.stderr_contains(needle),
            "Expected stderr to contain '{}', got:\n{}",
            needle,
            self.stderr_str()
        );
    }
}

static TUI_SESSION_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// TUI test harness using tmux.
pub struct TuiHarness {
    session_name: String,
    #[allow(dead_code)]
    data_path: PathBuf,
}

impl TuiHarness {
    /// Start the TUI in a tmux session.
    pub fn start(project: &TestProject) -> Self {
        Self::start_with_agent(project, None)
    }

    /// Start with a specific agent identity.
    pub fn start_as(project: &TestProject, agent: &str) -> Self {
        Self::start_with_agent(project, Some(agent))
    }

    fn start_with_agent(project: &TestProject, agent: Option<&str>) -> Self {
        let count = TUI_SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
        let session_name = format!("botbus-tui-{}-{}", std::process::id(), count);
        let bin = botbus_bin();

        let mut cmd = Command::new("tmux");
        cmd.args([
            "new-session",
            "-d",
            "-s",
            &session_name,
            "-x",
            "100",
            "-y",
            "30",
        ]);

        // Set env vars
        cmd.arg("-e");
        cmd.arg(format!("BOTBUS_DATA_DIR={}", project.data_path.display()));

        if let Some(agent_name) = agent {
            cmd.arg("-e");
            cmd.arg(format!("BOTBUS_AGENT={}", agent_name));
        }

        cmd.arg(&format!("{} ui", bin.display()));
        cmd.current_dir(&project.work_dir);

        let status = cmd.status().expect("Failed to start tmux");
        assert!(status.success(), "Failed to start tmux session");

        // Give TUI time to initialize
        std::thread::sleep(std::time::Duration::from_millis(500));

        Self {
            session_name,
            data_path: project.data_path.clone(),
        }
    }

    /// Capture the current pane content.
    pub fn capture(&self) -> String {
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &self.session_name, "-p"])
            .output()
            .expect("Failed to capture tmux pane");

        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// Send keys to the TUI.
    pub fn send_keys(&self, keys: &str) {
        let status = Command::new("tmux")
            .args(["send-keys", "-t", &self.session_name, keys])
            .status()
            .expect("Failed to send keys");

        assert!(status.success(), "Failed to send keys to tmux");

        // Small delay for TUI to process
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    /// Send a special key (Tab, Enter, Escape, etc).
    pub fn send_special(&self, key: &str) {
        self.send_keys(key);
    }

    /// Check if the session is still running.
    pub fn is_running(&self) -> bool {
        let output = Command::new("tmux")
            .args(["has-session", "-t", &self.session_name])
            .status();

        output.map(|s| s.success()).unwrap_or(false)
    }

    /// Wait for TUI to exit.
    pub fn wait_for_exit(&self, timeout_ms: u64) -> bool {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);

        while start.elapsed() < timeout {
            if !self.is_running() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        false
    }

    /// Assert that capture contains a string.
    pub fn assert_contains(&self, needle: &str) {
        let content = self.capture();
        assert!(
            content.contains(needle),
            "Expected TUI to contain '{}', got:\n{}",
            needle,
            content
        );
    }

    /// Kill the tmux session (cleanup).
    pub fn kill(&self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.session_name])
            .status();
    }
}

impl Drop for TuiHarness {
    fn drop(&mut self) {
        self.kill();
    }
}
