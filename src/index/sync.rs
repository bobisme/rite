use anyhow::{Context, Result};

use super::fts::SearchIndex;
use crate::core::message::{Message, MessageMeta};
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
    ///
    /// When encountering tombstone records, deletes the original message from the
    /// FTS index rather than indexing the tombstone itself.
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

        // Separate tombstones from regular messages
        let mut regular_messages = Vec::new();
        let mut deleted_ids = Vec::new();

        for msg in messages {
            if let Some(MessageMeta::Deleted { target_id, .. }) = &msg.meta {
                deleted_ids.push(target_id.to_string());
            } else {
                regular_messages.push(msg);
            }
        }

        // Delete tombstoned messages from FTS index
        for id in &deleted_ids {
            self.index.delete_message(id)?;
        }

        // Index remaining regular messages
        let count = self.index.index_messages(&regular_messages)?;
        self.index.set_sync_offset(channel, new_offset)?;

        Ok(count)
    }

    /// Rebuild the entire index from scratch.
    ///
    /// This performs a full rebuild:
    /// 1. Reads all messages from all JSONL files
    /// 2. Deduplicates by message ID (ULID)
    /// 3. Sorts by ULID (chronological order)
    /// 4. Clears existing FTS tables
    /// 5. Bulk inserts into FTS index using transactions
    pub fn rebuild(&mut self) -> Result<SyncStats> {
        use std::collections::HashMap;

        let channels = channels_dir();

        if !channels.exists() {
            return Ok(SyncStats::default());
        }

        let mut stats = SyncStats::default();
        let mut messages_by_id: HashMap<String, Message> = HashMap::new();

        // 1. Read all messages from all JSONL files
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(channel) = path.file_stem().and_then(|s| s.to_str())
            {
                stats.channels_synced += 1;

                // Read all messages from this channel (with deletion filtering)
                match crate::core::message::read_messages(&path) {
                    Ok(messages) => {
                        // 2. Deduplicate by message ID
                        for msg in messages {
                            messages_by_id.insert(msg.id.to_string(), msg);
                        }
                    }
                    Err(e) => {
                        stats.errors.push(format!("{}: {}", channel, e));
                    }
                }
            }
        }

        // 3. Sort by ULID (chronological order)
        let mut messages: Vec<Message> = messages_by_id.into_values().collect();
        messages.sort_by_key(|m| m.id);

        // 4. Clear existing FTS tables
        self.index.clear()?;

        // 5. Bulk insert into FTS index
        let count = self.index.index_messages(&messages)?;
        stats.messages_indexed = count;

        // Update sync offsets to the end of each file
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(channel) = path.file_stem().and_then(|s| s.to_str())
            {
                // Get the file size as the new offset
                let metadata = std::fs::metadata(&path)?;
                let offset = metadata.len();
                self.index.set_sync_offset(channel, offset)?;
            }
        }

        Ok(stats)
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
