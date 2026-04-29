use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::path::Path;

use super::schema::init_schema;
use crate::core::message::Message;

/// Escape a string for safe use in FTS5 queries.
///
/// FTS5 has special characters that need escaping:
/// - Double quotes: used for phrase queries
/// - Asterisk: used for prefix queries
/// - Parentheses: used for grouping
/// - AND, OR, NOT: boolean operators
/// - NEAR: proximity operator
/// - Colon: column filter
///
/// We wrap the term in double quotes and escape any internal quotes.
fn escape_fts5_term(term: &str) -> String {
    // Escape double quotes by doubling them, then wrap in quotes
    let escaped = term.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// A search result from the FTS index.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub channel: String,
    pub agent: String,
    pub body: String,
    pub ts: String,
    pub rank: f64,
}

/// Full-text search index backed by SQLite FTS5.
pub struct SearchIndex {
    conn: Connection,
}

impl SearchIndex {
    /// Open or create a search index at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open index: {}", path.display()))?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .with_context(|| "Failed to set WAL mode")?;

        init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Create an in-memory index (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn =
            Connection::open_in_memory().with_context(|| "Failed to open in-memory index")?;

        init_schema(&conn)?;

        Ok(Self { conn })
    }

    /// Index a message.
    pub fn index_message(&self, msg: &Message) -> Result<()> {
        let id = msg.id.to_string();
        let ts = msg.ts.to_rfc3339();

        self.conn
            .execute(
                "INSERT OR REPLACE INTO messages_fts (id, channel, agent, body, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, msg.channel, msg.agent, msg.body, ts],
            )
            .with_context(|| "Failed to insert into FTS")?;

        Ok(())
    }

    /// Index multiple messages in a transaction.
    pub fn index_messages(&mut self, messages: &[Message]) -> Result<usize> {
        if messages.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.transaction()?;

        for msg in messages {
            let id = msg.id.to_string();
            let ts = msg.ts.to_rfc3339();

            tx.execute(
                "INSERT OR REPLACE INTO messages_fts (id, channel, agent, body, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, msg.channel, msg.agent, msg.body, ts],
            )?;
        }

        tx.commit()?;

        Ok(messages.len())
    }

    /// Search for messages matching a query.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, channel, agent, body, ts, bm25(messages_fts) as rank
            FROM messages_fts
            WHERE messages_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            "#,
        )?;

        let results = stmt
            .query_map(params![query, limit as i64], |row| {
                Ok(SearchResult {
                    id: row.get(0)?,
                    channel: row.get(1)?,
                    agent: row.get(2)?,
                    body: row.get(3)?,
                    ts: row.get(4)?,
                    rank: row.get(5)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Search within a specific channel.
    pub fn search_channel(
        &self,
        query: &str,
        channel: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // Combine query with channel filter using FTS5 AND syntax
        // Escape the channel name to prevent FTS5 injection
        let fts_query = format!("{} AND channel:{}", query, escape_fts5_term(channel));
        self.search(&fts_query, limit)
    }

    /// Search messages from a specific agent.
    pub fn search_from(&self, query: &str, agent: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Combine query with agent filter using FTS5 AND syntax
        // Escape the agent name to prevent FTS5 injection
        let fts_query = format!("{} AND agent:{}", query, escape_fts5_term(agent));
        self.search(&fts_query, limit)
    }

    /// Search within a specific channel and from a specific agent.
    pub fn search_channel_from(
        &self,
        query: &str,
        channel: &str,
        agent: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let fts_query = format!(
            "{} AND channel:{} AND agent:{}",
            query,
            escape_fts5_term(channel),
            escape_fts5_term(agent)
        );
        self.search(&fts_query, limit)
    }

    /// Get sync offset for a channel.
    pub fn get_sync_offset(&self, channel: &str) -> Result<u64> {
        let offset: Option<i64> = self
            .conn
            .query_row(
                "SELECT offset FROM sync_state WHERE channel = ?1",
                params![channel],
                |row| row.get(0),
            )
            .ok();

        Ok(offset.unwrap_or(0) as u64)
    }

    /// Set sync offset for a channel.
    pub fn set_sync_offset(&self, channel: &str, offset: u64) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO sync_state (channel, offset, last_sync) VALUES (?1, ?2, ?3)",
            params![channel, offset as i64, now],
        )?;
        Ok(())
    }

    /// Get the total number of indexed messages.
    pub fn message_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages_fts", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Delete a specific message from the FTS index by its ULID ID.
    pub fn delete_message(&self, id: &str) -> Result<bool> {
        let changes = self
            .conn
            .execute("DELETE FROM messages_fts WHERE id = ?1", params![id])
            .with_context(|| format!("Failed to delete message {} from FTS", id))?;

        Ok(changes > 0)
    }

    /// Clear all messages from the FTS index.
    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM messages_fts", [])
            .with_context(|| "Failed to clear FTS index")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ulid::Ulid;

    #[test]
    fn test_escape_fts5_term() {
        // Simple term
        assert_eq!(escape_fts5_term("hello"), "\"hello\"");

        // Term with double quotes
        assert_eq!(escape_fts5_term("say \"hello\""), "\"say \"\"hello\"\"\"");

        // Term with FTS5 operators (should be neutralized by quoting)
        assert_eq!(escape_fts5_term("foo AND bar"), "\"foo AND bar\"");
        assert_eq!(escape_fts5_term("foo OR bar"), "\"foo OR bar\"");
        assert_eq!(escape_fts5_term("NOT foo"), "\"NOT foo\"");

        // Term with special characters
        assert_eq!(escape_fts5_term("prefix*"), "\"prefix*\"");
        assert_eq!(escape_fts5_term("(grouped)"), "\"(grouped)\"");
        assert_eq!(escape_fts5_term("col:value"), "\"col:value\"");
    }

    fn make_message(channel: &str, agent: &str, body: &str) -> Message {
        Message {
            ts: Utc::now(),
            id: Ulid::new(),
            agent: agent.to_string(),
            channel: channel.to_string(),
            body: body.to_string(),
            mentions: vec![],
            labels: vec![],
            attachments: vec![],
            meta: None,
        }
    }

    #[test]
    fn test_index_and_search() {
        let mut index = SearchIndex::open_in_memory().unwrap();

        let messages = vec![
            make_message("general", "Alice", "Hello world"),
            make_message("general", "Bob", "Working on authentication"),
            make_message("backend", "Alice", "Fixed the bug in auth module"),
        ];

        index.index_messages(&messages).unwrap();

        // Search for "auth" in body field
        let results = index.search("body:auth*", 10).unwrap();
        assert_eq!(results.len(), 2);

        // Search in specific channel
        let results = index.search_channel("body:auth*", "backend", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].channel, "backend");
    }

    #[test]
    fn test_search_from_agent() {
        let mut index = SearchIndex::open_in_memory().unwrap();

        let messages = vec![
            make_message("general", "Alice", "Hello from Alice"),
            make_message("general", "Bob", "Hello from Bob"),
        ];

        index.index_messages(&messages).unwrap();

        let results = index.search_from("body:Hello", "Alice", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent, "Alice");
    }

    #[test]
    fn test_search_channel_from_agent() {
        let mut index = SearchIndex::open_in_memory().unwrap();

        let messages = vec![
            make_message("general", "Alice", "Investigating auth"),
            make_message("general", "Bob", "Investigating auth"),
            make_message("backend", "Alice", "Investigating auth"),
        ];

        index.index_messages(&messages).unwrap();

        let results = index
            .search_channel_from("body:Investigating", "general", "Alice", 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].channel, "general");
        assert_eq!(results[0].agent, "Alice");
    }

    #[test]
    fn test_sync_offset() {
        let index = SearchIndex::open_in_memory().unwrap();

        assert_eq!(index.get_sync_offset("general").unwrap(), 0);

        index.set_sync_offset("general", 1234).unwrap();
        assert_eq!(index.get_sync_offset("general").unwrap(), 1234);
    }

    #[test]
    fn test_message_count() {
        let mut index = SearchIndex::open_in_memory().unwrap();

        assert_eq!(index.message_count().unwrap(), 0);

        let messages = vec![
            make_message("general", "Alice", "One"),
            make_message("general", "Bob", "Two"),
        ];

        index.index_messages(&messages).unwrap();
        assert_eq!(index.message_count().unwrap(), 2);
    }
}
