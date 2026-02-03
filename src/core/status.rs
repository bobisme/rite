use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// An agent's status (presence + message).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusEntry {
    /// Timestamp when status was set
    pub ts: DateTime<Utc>,

    /// Agent name
    pub agent: String,

    /// Status message (up to 32 chars)
    pub message: String,

    /// When this status expires
    pub expires_at: DateTime<Utc>,

    /// Whether this status is still active (false = cleared)
    pub active: bool,
}

impl AgentStatusEntry {
    pub fn new(agent: impl Into<String>, message: impl Into<String>, ttl_secs: u64) -> Self {
        let now = Utc::now();
        Self {
            ts: now,
            agent: agent.into(),
            message: message.into(),
            expires_at: now + Duration::seconds(ttl_secs as i64),
            active: true,
        }
    }

    pub fn clear(agent: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            ts: now,
            agent: agent.into(),
            message: String::new(),
            expires_at: now,
            active: false,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.active && Utc::now() < self.expires_at
    }

    pub fn is_recently_expired(&self) -> bool {
        let now = Utc::now();
        self.active && now >= self.expires_at && (now - self.expires_at).num_seconds() < 86400
    }
}
