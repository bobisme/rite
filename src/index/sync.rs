use anyhow::{Context, Result};

use super::fts::SearchIndex;
use crate::core::message::Message;
use crate::core::project::{channels_dir, index_path};
use crate::storage::jsonl::read_records_from_offset;

/// Syncs JSONL logs to the FTS index.
pub struct IndexSyncer {
    index: SearchIndex,
}

impl IndexSyncer {
    /// Create a new syncer.
    pub fn new() -> Result<Self> {
        let idx_path = index_path();
        let index = SearchIndex::open(&idx_path)
            .with_context(|| format!("Failed to open index at {}", idx_path.display()))?;

        Ok(Self { index })
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
        let channels = channels_dir();

        if !channels.exists() {
            return Ok(SyncStats::default());
        }

        let mut stats = SyncStats::default();

        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(channel) = path.file_stem().and_then(|s| s.to_str())
            {
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

        Ok(stats)
    }

    /// Sync a specific channel incrementally.
    pub fn sync_channel(&mut self, channel: &str) -> Result<usize> {
        let path = channels_dir().join(format!("{}.jsonl", channel));

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
        let channels = channels_dir();

        if !channels.exists() {
            return Ok(SyncStats::default());
        }

        // Reset offsets and re-sync
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(channel) = path.file_stem().and_then(|s| s.to_str())
            {
                self.index.set_sync_offset(channel, 0)?;
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
    // Integration tests moved to tests/integration/ since they require
    // global data directory mocking
}
