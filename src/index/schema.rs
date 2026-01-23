use anyhow::{Context, Result};
use rusqlite::Connection;

/// SQL schema for the FTS index.
pub const SCHEMA: &str = r#"
-- Messages FTS table (stores content for search results)
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    id,
    channel,
    agent,
    body,
    ts
);

-- Sync state tracking
CREATE TABLE IF NOT EXISTS sync_state (
    channel TEXT PRIMARY KEY,
    offset INTEGER NOT NULL DEFAULT 0,
    last_sync TEXT NOT NULL
);
"#;

/// Initialize the database schema.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)
        .with_context(|| "Failed to initialize FTS schema")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_schema() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"messages_fts".to_string()));
        assert!(tables.contains(&"sync_state".to_string()));
    }
}
