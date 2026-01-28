//! Per-agent state storage.
//!
//! Each agent has its own state file at `.botbus/agents/<AgentName>/state.json`
//! to avoid conflicts when multiple agents share a project.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Per-agent state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentState {
    /// Read cursor offsets per channel (byte offsets into JSONL files)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub read_offsets: HashMap<String, u64>,

    /// Last read message ID per channel (for robustness across compaction)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub last_read_ids: HashMap<String, String>,

    /// Last read timestamp per channel
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub last_read_times: HashMap<String, DateTime<Utc>>,

    /// Channels this agent is subscribed to
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscribed_channels: Vec<String>,
}

impl AgentState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Get the path to an agent's state file.
pub fn agent_state_path(project_root: &Path, agent_name: &str) -> PathBuf {
    project_root
        .join(".botbus")
        .join("agents")
        .join(agent_name)
        .join("state.json")
}

/// Manager for per-agent state with file locking.
pub struct AgentStateManager {
    path: PathBuf,
}

impl AgentStateManager {
    /// Create a new manager for the given agent.
    pub fn new(project_root: &Path, agent_name: &str) -> Self {
        Self {
            path: agent_state_path(project_root, agent_name),
        }
    }

    /// Create from an explicit path (for testing).
    pub fn from_path(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Load the agent's state from disk.
    pub fn load(&self) -> Result<AgentState> {
        if !self.path.exists() {
            return Ok(AgentState::default());
        }

        let file = File::open(&self.path)
            .with_context(|| format!("Failed to open agent state: {}", self.path.display()))?;

        file.lock_shared()
            .with_context(|| "Failed to acquire shared lock on agent state")?;

        let mut contents = String::new();
        let mut reader = std::io::BufReader::new(&file);
        reader
            .read_to_string(&mut contents)
            .with_context(|| "Failed to read agent state")?;

        if contents.trim().is_empty() {
            return Ok(AgentState::default());
        }

        let state: AgentState =
            serde_json::from_str(&contents).with_context(|| "Failed to parse agent state")?;

        Ok(state)
    }

    /// Save the agent's state to disk.
    pub fn save(&self, state: &AgentState) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .with_context(|| {
                format!(
                    "Failed to open agent state for writing: {}",
                    self.path.display()
                )
            })?;

        file.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on agent state")?;

        let json = serde_json::to_string_pretty(state)
            .with_context(|| "Failed to serialize agent state")?;

        let mut writer = std::io::BufWriter::new(&file);
        writer
            .write_all(json.as_bytes())
            .with_context(|| "Failed to write agent state")?;

        writer.flush()?;
        file.sync_all()?;

        Ok(())
    }

    /// Update the state atomically using a closure.
    ///
    /// Holds an exclusive lock across the entire read-modify-write operation
    /// to prevent race conditions between concurrent updates.
    pub fn update<F>(&self, f: F) -> Result<AgentState>
    where
        F: FnOnce(&mut AgentState),
    {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        // Open file with read+write, creating if needed
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&self.path)
            .with_context(|| {
                format!(
                    "Failed to open agent state for update: {}",
                    self.path.display()
                )
            })?;

        // Acquire exclusive lock - held for entire read-modify-write
        file.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on agent state")?;

        // Read current contents
        let mut contents = String::new();
        let mut reader = std::io::BufReader::new(&file);
        reader
            .read_to_string(&mut contents)
            .with_context(|| "Failed to read agent state")?;

        // Parse state (default if empty/missing)
        let mut state: AgentState = if contents.trim().is_empty() {
            AgentState::default()
        } else {
            serde_json::from_str(&contents).with_context(|| "Failed to parse agent state")?
        };

        // Apply the update
        f(&mut state);

        // Serialize
        let json = serde_json::to_string_pretty(&state)
            .with_context(|| "Failed to serialize agent state")?;

        // Truncate file and write back (file position is at end after read)
        use std::io::Seek;
        let mut file_ref = &file;
        file_ref.seek(std::io::SeekFrom::Start(0))?;
        file.set_len(0)?;

        let mut writer = std::io::BufWriter::new(&file);
        writer
            .write_all(json.as_bytes())
            .with_context(|| "Failed to write agent state")?;

        writer.flush()?;
        file.sync_all()?;

        // Lock released on drop
        Ok(state)
    }

    /// Get the read offset for a channel.
    pub fn get_read_offset(&self, channel: &str) -> Result<u64> {
        Ok(self.load()?.read_offsets.get(channel).copied().unwrap_or(0))
    }

    /// Set the read offset for a channel.
    pub fn set_read_offset(&self, channel: &str, offset: u64) -> Result<()> {
        self.update(|s| {
            s.read_offsets.insert(channel.to_string(), offset);
        })?;
        Ok(())
    }

    /// Get the last read message ID for a channel.
    pub fn get_last_read_id(&self, channel: &str) -> Result<Option<String>> {
        Ok(self.load()?.last_read_ids.get(channel).cloned())
    }

    /// Set the last read message ID for a channel.
    pub fn set_last_read_id(&self, channel: &str, id: &str) -> Result<()> {
        self.update(|s| {
            s.last_read_ids.insert(channel.to_string(), id.to_string());
        })?;
        Ok(())
    }

    /// Mark a channel as read up to a specific offset and message ID.
    pub fn mark_read(&self, channel: &str, offset: u64, last_id: Option<&str>) -> Result<()> {
        self.update(|s| {
            s.read_offsets.insert(channel.to_string(), offset);
            s.last_read_times.insert(channel.to_string(), Utc::now());
            if let Some(id) = last_id {
                s.last_read_ids.insert(channel.to_string(), id.to_string());
            }
        })?;
        Ok(())
    }

    /// Get read cursor info for a channel.
    pub fn get_read_cursor(&self, channel: &str) -> Result<ReadCursor> {
        let state = self.load()?;
        Ok(ReadCursor {
            offset: state.read_offsets.get(channel).copied().unwrap_or(0),
            last_id: state.last_read_ids.get(channel).cloned(),
            last_time: state.last_read_times.get(channel).copied(),
        })
    }

    /// Subscribe to a channel.
    pub fn subscribe(&self, channel: &str) -> Result<bool> {
        let mut added = false;
        self.update(|s| {
            if !s.subscribed_channels.contains(&channel.to_string()) {
                s.subscribed_channels.push(channel.to_string());
                added = true;
            }
        })?;
        Ok(added)
    }

    /// Unsubscribe from a channel.
    pub fn unsubscribe(&self, channel: &str) -> Result<bool> {
        let mut removed = false;
        self.update(|s| {
            if let Some(pos) = s.subscribed_channels.iter().position(|c| c == channel) {
                s.subscribed_channels.remove(pos);
                removed = true;
            }
        })?;
        Ok(removed)
    }

    /// Get list of subscribed channels.
    pub fn get_subscribed_channels(&self) -> Result<Vec<String>> {
        Ok(self.load()?.subscribed_channels)
    }
}

/// Read cursor information for a channel.
#[derive(Debug, Clone, Default)]
pub struct ReadCursor {
    /// Byte offset into the channel file
    pub offset: u64,
    /// Last read message ID (ULID)
    pub last_id: Option<String>,
    /// Timestamp when mark-read was called
    pub last_time: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn test_agent_state_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("agent_state.json");
        let manager = AgentStateManager::from_path(&path);

        let mut state = AgentState::new();
        state.read_offsets.insert("general".to_string(), 1234);
        state
            .last_read_ids
            .insert("general".to_string(), "01ABC".to_string());

        manager.save(&state).unwrap();
        let loaded = manager.load().unwrap();

        assert_eq!(loaded.read_offsets.get("general"), Some(&1234));
        assert_eq!(
            loaded.last_read_ids.get("general"),
            Some(&"01ABC".to_string())
        );
    }

    #[test]
    fn test_load_nonexistent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nonexistent.json");
        let manager = AgentStateManager::from_path(&path);

        let state = manager.load().unwrap();
        assert!(state.read_offsets.is_empty());
    }

    #[test]
    fn test_set_read_offset() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let manager = AgentStateManager::from_path(&path);

        manager.set_read_offset("general", 5678).unwrap();
        assert_eq!(manager.get_read_offset("general").unwrap(), 5678);
    }

    #[test]
    fn test_mark_read() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let manager = AgentStateManager::from_path(&path);

        manager.mark_read("general", 9999, Some("01XYZ")).unwrap();

        let cursor = manager.get_read_cursor("general").unwrap();
        assert_eq!(cursor.offset, 9999);
        assert_eq!(cursor.last_id, Some("01XYZ".to_string()));
        assert!(cursor.last_time.is_some());
    }

    #[test]
    fn test_per_agent_isolation() {
        let temp = TempDir::new().unwrap();
        let project = temp.path();

        let agent1 = AgentStateManager::new(project, "Agent1");
        let agent2 = AgentStateManager::new(project, "Agent2");

        agent1.set_read_offset("general", 100).unwrap();
        agent2.set_read_offset("general", 200).unwrap();

        // Each agent has its own offset
        assert_eq!(agent1.get_read_offset("general").unwrap(), 100);
        assert_eq!(agent2.get_read_offset("general").unwrap(), 200);
    }

    /// Stress test for concurrent agent state updates (bd-k7r).
    ///
    /// Spawns multiple threads that concurrently increment a read offset.
    /// Verifies that no updates are lost due to race conditions.
    #[test]
    fn test_concurrent_agent_state_updates() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("agent_state.json");
        let manager = Arc::new(AgentStateManager::from_path(&path));

        // Initialize the counter
        manager
            .update(|s| {
                s.read_offsets.insert("counter".to_string(), 0);
            })
            .unwrap();

        const NUM_THREADS: usize = 10;
        const INCREMENTS_PER_THREAD: usize = 50;

        let mut handles = Vec::new();
        for _ in 0..NUM_THREADS {
            let manager = Arc::clone(&manager);
            let handle = std::thread::spawn(move || {
                for _ in 0..INCREMENTS_PER_THREAD {
                    manager
                        .update(|s| {
                            let current = s.read_offsets.get("counter").copied().unwrap_or(0);
                            s.read_offsets.insert("counter".to_string(), current + 1);
                        })
                        .unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Verify no updates were lost
        let state = manager.load().unwrap();
        let final_count = state.read_offsets.get("counter").copied().unwrap_or(0);
        let expected = (NUM_THREADS * INCREMENTS_PER_THREAD) as u64;

        assert_eq!(
            final_count, expected,
            "Expected {} increments but got {} - updates were lost!",
            expected, final_count
        );
    }

    #[test]
    fn test_subscribe_channel() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let manager = AgentStateManager::from_path(&path);

        // Subscribe to a channel
        let added = manager.subscribe("general").unwrap();
        assert!(added);

        // Verify subscription
        let channels = manager.get_subscribed_channels().unwrap();
        assert_eq!(channels, vec!["general"]);

        // Subscribe to same channel again (should be idempotent)
        let added = manager.subscribe("general").unwrap();
        assert!(!added);

        let channels = manager.get_subscribed_channels().unwrap();
        assert_eq!(channels, vec!["general"]);
    }

    #[test]
    fn test_unsubscribe_channel() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let manager = AgentStateManager::from_path(&path);

        // Subscribe to channels
        manager.subscribe("general").unwrap();
        manager.subscribe("dev").unwrap();

        // Unsubscribe from one
        let removed = manager.unsubscribe("general").unwrap();
        assert!(removed);

        let channels = manager.get_subscribed_channels().unwrap();
        assert_eq!(channels, vec!["dev"]);

        // Unsubscribe from non-existent (should be idempotent)
        let removed = manager.unsubscribe("general").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_multiple_subscriptions() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let manager = AgentStateManager::from_path(&path);

        // Subscribe to multiple channels
        manager.subscribe("general").unwrap();
        manager.subscribe("dev").unwrap();
        manager.subscribe("ops").unwrap();

        let channels = manager.get_subscribed_channels().unwrap();
        assert_eq!(channels.len(), 3);
        assert!(channels.contains(&"general".to_string()));
        assert!(channels.contains(&"dev".to_string()));
        assert!(channels.contains(&"ops".to_string()));
    }
}
