use anyhow::{Context, Result};
use std::path::Path;

use super::fts::SearchIndex;
use crate::core::message::Message;
use crate::core::project::{channels_dir, index_path};
use crate::storage::jsonl::read_records_from_offset;

/// Syncs JSONL logs to the FTS index.
pub struct IndexSyncer {
    index: SearchIndex,
    project_root: std::path::PathBuf,
}

impl IndexSyncer {
    /// Create a new syncer for the given project.
    pub fn new(project_root: &Path) -> Result<Self> {
        let idx_path = index_path(project_root);
        let index = SearchIndex::open(&idx_path)
            .with_context(|| format!("Failed to open index at {}", idx_path.display()))?;

        Ok(Self {
            index,
            project_root: project_root.to_path_buf(),
        })
    }

    /// Get a reference to the underlying index.
    pub fn index(&self) -> &SearchIndex {
        &self.index
    }

    /// Get a mutable reference to the underlying index.
    pub fn index_mut(&mut self) -> &mut SearchIndex {
        &mut self.index
    }

    /// Sync all channels incrementally.
    pub fn sync_all(&mut self) -> Result<SyncStats> {
        let channels = channels_dir(&self.project_root);

        if !channels.exists() {
            return Ok(SyncStats::default());
        }

        let mut stats = SyncStats::default();

        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(channel) = path.file_stem().and_then(|s| s.to_str()) {
                    match self.sync_channel(channel) {
                        Ok(count) => {
                            stats.messages_indexed += count;
                            stats.channels_synced += 1;
                        }
                        Err(e) => {
                            stats.errors.push(format!("{}: {}", channel, e));
                        }
                    }
                }
            }
        }

        Ok(stats)
    }

    /// Sync a specific channel incrementally.
    pub fn sync_channel(&mut self, channel: &str) -> Result<usize> {
        let path = channels_dir(&self.project_root).join(format!("{}.jsonl", channel));

        if !path.exists() {
            return Ok(0);
        }

        let offset = self.index.get_sync_offset(channel)?;
        let (messages, new_offset): (Vec<Message>, u64) = read_records_from_offset(&path, offset)?;

        if messages.is_empty() {
            return Ok(0);
        }

        let count = self.index.index_messages(&messages)?;
        self.index.set_sync_offset(channel, new_offset)?;

        Ok(count)
    }

    /// Rebuild the entire index from scratch.
    pub fn rebuild(&mut self) -> Result<SyncStats> {
        // Reset all offsets
        let channels = channels_dir(&self.project_root);

        if !channels.exists() {
            return Ok(SyncStats::default());
        }

        // Reset offsets and re-sync
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(channel) = path.file_stem().and_then(|s| s.to_str()) {
                    self.index.set_sync_offset(channel, 0)?;
                }
            }
        }

        self.sync_all()
    }
}

/// Statistics from a sync operation.
#[derive(Debug, Default)]
pub struct SyncStats {
    pub channels_synced: usize,
    pub messages_indexed: usize,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{init, register, send};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        register::run(Some("Indexer".to_string()), None, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_sync_channel() {
        let temp = setup();

        // Send some messages
        send::run(
            "general".to_string(),
            "Hello".to_string(),
            None,
            temp.path(),
        )
        .unwrap();
        send::run(
            "general".to_string(),
            "World".to_string(),
            None,
            temp.path(),
        )
        .unwrap();

        // Sync
        let mut syncer = IndexSyncer::new(temp.path()).unwrap();
        let stats = syncer.sync_all().unwrap();

        assert!(stats.messages_indexed >= 2);
        assert!(stats.errors.is_empty());

        // Search should work
        let results = syncer.index().search("body:Hello", 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn test_incremental_sync() {
        let temp = setup();

        send::run(
            "general".to_string(),
            "First".to_string(),
            None,
            temp.path(),
        )
        .unwrap();

        let mut syncer = IndexSyncer::new(temp.path()).unwrap();
        let stats1 = syncer.sync_all().unwrap();
        let count1 = stats1.messages_indexed;

        // Send more messages
        send::run(
            "general".to_string(),
            "Second".to_string(),
            None,
            temp.path(),
        )
        .unwrap();

        // Sync again - should only index new messages
        let stats2 = syncer.sync_all().unwrap();
        assert_eq!(stats2.messages_indexed, 1);

        // Total should be correct
        assert_eq!(syncer.index().message_count().unwrap(), count1 + 1);
    }
}
