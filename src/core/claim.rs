use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A claim on files/patterns for editing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileClaim {
    /// Timestamp when claim was created
    pub ts: DateTime<Utc>,

    /// Unique identifier
    pub id: Ulid,

    /// Agent that owns this claim
    pub agent: String,

    /// Glob patterns being claimed (e.g., "src/auth/**/*.rs")
    pub patterns: Vec<String>,

    /// When the claim expires (UTC)
    pub expires_at: DateTime<Utc>,

    /// Whether this claim is still active
    pub active: bool,

    /// Event type (created, released, expired)
    pub event: ClaimEvent,
}

impl FileClaim {
    /// Create a new file claim with the given TTL in seconds.
    pub fn new(agent: impl Into<String>, patterns: Vec<String>, ttl_secs: u64) -> Self {
        let now = Utc::now();
        Self {
            ts: now,
            id: Ulid::new(),
            agent: agent.into(),
            patterns,
            expires_at: now + Duration::seconds(ttl_secs as i64),
            active: true,
            event: ClaimEvent::Created,
        }
    }

    /// Check if this claim has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    /// Check if this claim is currently valid (active and not expired).
    pub fn is_valid(&self) -> bool {
        self.active && !self.is_expired()
    }

    /// Create a release event for this claim.
    pub fn release(&self) -> Self {
        Self {
            ts: Utc::now(),
            id: self.id,
            agent: self.agent.clone(),
            patterns: self.patterns.clone(),
            expires_at: self.expires_at,
            active: false,
            event: ClaimEvent::Released,
        }
    }

    /// Create an expiration event for this claim.
    pub fn expire(&self) -> Self {
        Self {
            ts: Utc::now(),
            id: self.id,
            agent: self.agent.clone(),
            patterns: self.patterns.clone(),
            expires_at: self.expires_at,
            active: false,
            event: ClaimEvent::Expired,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimEvent {
    Created,
    Released,
    Expired,
}

/// Represents a conflict between claims.
#[derive(Debug, Clone)]
pub struct ClaimConflict {
    pub your_pattern: String,
    pub existing_pattern: String,
    pub holder: String,
    pub expires_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_roundtrip() {
        let claim = FileClaim::new("Agent1", vec!["src/**/*.rs".to_string()], 3600);

        let json = serde_json::to_string(&claim).unwrap();
        let parsed: FileClaim = serde_json::from_str(&json).unwrap();

        assert_eq!(claim.id, parsed.id);
        assert_eq!(claim.agent, parsed.agent);
        assert_eq!(claim.patterns, parsed.patterns);
    }

    #[test]
    fn test_claim_validity() {
        let claim = FileClaim::new("Agent1", vec!["*.rs".to_string()], 3600);
        assert!(claim.is_valid());
        assert!(!claim.is_expired());

        let released = claim.release();
        assert!(!released.is_valid());
        assert_eq!(released.event, ClaimEvent::Released);
    }

    #[test]
    fn test_claim_expiration() {
        // Create a claim that's already expired
        let mut claim = FileClaim::new("Agent1", vec!["*.rs".to_string()], 0);
        claim.expires_at = Utc::now() - Duration::seconds(1);

        assert!(claim.is_expired());
        assert!(!claim.is_valid());
    }
}
