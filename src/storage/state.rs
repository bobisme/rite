use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

/// Project-level state stored in state.json.
///
/// This is mutable state that can be updated (unlike JSONL logs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// Current agent identity for this project
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_agent: Option<String>,

    /// Read cursors per channel (byte offsets for incremental reading)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub channel_offsets: HashMap<String, u64>,

    /// Last seen message timestamp per channel
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub last_seen: HashMap<String, DateTime<Utc>>,

    /// Index sync offset per channel
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub index_offsets: HashMap<String, u64>,
}

impl State {
    /// Create a new empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a state with the current agent set.
    pub fn with_agent(agent: impl Into<String>) -> Self {
        Self {
            current_agent: Some(agent.into()),
            ..Default::default()
        }
    }
}

/// Wrapper for loading/saving project state with file locking.
pub struct ProjectState {
    path: std::path::PathBuf,
}

impl ProjectState {
    /// Create a new ProjectState for the given path.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Load the state from disk.
    ///
    /// Returns default state if the file doesn't exist.
    pub fn load(&self) -> Result<State> {
        if !self.path.exists() {
            return Ok(State::default());
        }

        let file = File::open(&self.path)
            .with_context(|| format!("Failed to open state file: {}", self.path.display()))?;

        file.lock_shared()
            .with_context(|| "Failed to acquire shared lock on state file")?;

        let mut contents = String::new();
        let mut reader = std::io::BufReader::new(&file);
        reader
            .read_to_string(&mut contents)
            .with_context(|| "Failed to read state file")?;

        if contents.trim().is_empty() {
            return Ok(State::default());
        }

        let state: State =
            serde_json::from_str(&contents).with_context(|| "Failed to parse state file")?;

        Ok(state)
    }

    /// Save the state to disk.
    pub fn save(&self, state: &State) -> Result<()> {
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
                    "Failed to open state file for writing: {}",
                    self.path.display()
                )
            })?;

        file.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on state file")?;

        let json =
            serde_json::to_string_pretty(state).with_context(|| "Failed to serialize state")?;

        let mut writer = std::io::BufWriter::new(&file);
        writer
            .write_all(json.as_bytes())
            .with_context(|| "Failed to write state file")?;

        writer.flush()?;
        file.sync_all()?;

        Ok(())
    }

    /// Update the state atomically using a closure.
    ///
    /// Holds an exclusive lock across the entire read-modify-write operation
    /// to prevent race conditions between concurrent updates.
    pub fn update<F>(&self, f: F) -> Result<State>
    where
        F: FnOnce(&mut State),
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
            .truncate(false)
            .open(&self.path)
            .with_context(|| {
                format!(
                    "Failed to open state file for update: {}",
                    self.path.display()
                )
            })?;

        // Acquire exclusive lock - held for entire read-modify-write
        file.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on state file")?;

        // Read current contents
        let mut contents = String::new();
        let mut reader = std::io::BufReader::new(&file);
        reader
            .read_to_string(&mut contents)
            .with_context(|| "Failed to read state file")?;

        // Parse state (default if empty/missing)
        let mut state: State = if contents.trim().is_empty() {
            State::default()
        } else {
            serde_json::from_str(&contents).with_context(|| "Failed to parse state file")?
        };

        // Apply the update
        f(&mut state);

        // Serialize
        let json =
            serde_json::to_string_pretty(&state).with_context(|| "Failed to serialize state")?;

        // Truncate file and write back (file position is at end after read)
        use std::io::Seek;
        let mut file_ref = &file;
        file_ref.seek(std::io::SeekFrom::Start(0))?;
        file.set_len(0)?;

        let mut writer = std::io::BufWriter::new(&file);
        writer
            .write_all(json.as_bytes())
            .with_context(|| "Failed to write state file")?;

        writer.flush()?;
        file.sync_all()?;

        // Lock released on drop
        Ok(state)
    }

    /// Get the current agent name.
    pub fn current_agent(&self) -> Result<Option<String>> {
        Ok(self.load()?.current_agent)
    }

    /// Set the current agent name.
    pub fn set_current_agent(&self, agent: impl Into<String>) -> Result<()> {
        self.update(|s| {
            s.current_agent = Some(agent.into());
        })?;
        Ok(())
    }

    /// Update channel offset for incremental reading.
    pub fn set_channel_offset(&self, channel: &str, offset: u64) -> Result<()> {
        self.update(|s| {
            s.channel_offsets.insert(channel.to_string(), offset);
        })?;
        Ok(())
    }

    /// Get channel offset for incremental reading.
    pub fn get_channel_offset(&self, channel: &str) -> Result<u64> {
        Ok(self
            .load()?
            .channel_offsets
            .get(channel)
            .copied()
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn test_state_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let ps = ProjectState::new(&path);

        let mut state = State::new();
        state.current_agent = Some("TestAgent".to_string());
        state.channel_offsets.insert("general".to_string(), 1234);

        ps.save(&state).unwrap();
        let loaded = ps.load().unwrap();

        assert_eq!(loaded.current_agent, Some("TestAgent".to_string()));
        assert_eq!(loaded.channel_offsets.get("general"), Some(&1234));
    }

    #[test]
    fn test_load_nonexistent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("nonexistent.json");
        let ps = ProjectState::new(&path);

        let state = ps.load().unwrap();
        assert!(state.current_agent.is_none());
    }

    #[test]
    fn test_update() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let ps = ProjectState::new(&path);

        ps.update(|s| {
            s.current_agent = Some("Agent1".to_string());
        })
        .unwrap();

        ps.update(|s| {
            s.channel_offsets.insert("general".to_string(), 100);
        })
        .unwrap();

        let state = ps.load().unwrap();
        assert_eq!(state.current_agent, Some("Agent1".to_string()));
        assert_eq!(state.channel_offsets.get("general"), Some(&100));
    }

    #[test]
    fn test_set_current_agent() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let ps = ProjectState::new(&path);

        ps.set_current_agent("MyAgent").unwrap();
        assert_eq!(ps.current_agent().unwrap(), Some("MyAgent".to_string()));
    }

    /// Stress test for concurrent state updates (bd-k7r).
    ///
    /// Spawns multiple threads that concurrently increment a counter stored
    /// in channel_offsets. Verifies that no updates are lost due to race conditions.
    #[test]
    fn test_concurrent_state_updates() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        let ps = Arc::new(ProjectState::new(&path));

        // Initialize the counter
        ps.update(|s| {
            s.channel_offsets.insert("counter".to_string(), 0);
        })
        .unwrap();

        const NUM_THREADS: usize = 10;
        const INCREMENTS_PER_THREAD: usize = 50;

        let mut handles = Vec::new();
        for _ in 0..NUM_THREADS {
            let ps = Arc::clone(&ps);
            let handle = std::thread::spawn(move || {
                for _ in 0..INCREMENTS_PER_THREAD {
                    ps.update(|s| {
                        let current = s.channel_offsets.get("counter").copied().unwrap_or(0);
                        s.channel_offsets.insert("counter".to_string(), current + 1);
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
        let state = ps.load().unwrap();
        let final_count = state.channel_offsets.get("counter").copied().unwrap_or(0);
        let expected = (NUM_THREADS * INCREMENTS_PER_THREAD) as u64;

        assert_eq!(
            final_count, expected,
            "Expected {} increments but got {} - updates were lost!",
            expected, final_count
        );
    }
}
