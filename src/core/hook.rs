use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A channel hook that triggers a command when a message is sent to a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// Terse ID (e.g., "hk-a3x")
    pub id: String,

    /// Channel that triggers this hook
    pub channel: String,

    /// Condition that must be met for the hook to fire
    pub condition: HookCondition,

    /// Command to execute (no shell — executed via execvp)
    pub command: Vec<String>,

    /// Working directory for the command
    pub cwd: PathBuf,

    /// Minimum seconds between firings
    pub cooldown_secs: u64,

    /// Last time this hook was fired
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fired: Option<DateTime<Utc>>,

    /// When this hook was created
    pub created_at: DateTime<Utc>,

    /// Agent that created the hook
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Whether this hook is active
    pub active: bool,
}

/// Condition that must be met for a hook to fire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookCondition {
    /// Fire when no active claim holds the given pattern.
    ClaimAvailable { pattern: String },
}

/// Audit record for a hook evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookFiring {
    /// When the evaluation happened
    pub ts: DateTime<Utc>,

    /// Hook that was evaluated
    pub hook_id: String,

    /// Channel that triggered the evaluation
    pub channel: String,

    /// Message ID that triggered the evaluation
    pub message_id: String,

    /// Whether the condition passed
    pub condition_result: bool,

    /// Whether the command was actually executed
    pub executed: bool,

    /// Reason the hook was skipped (cooldown, condition failed, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Hook {
    /// Generate a terse hook ID using ULID.
    ///
    /// Takes last 3 chars of ULID (Crockford base32), lowercased, prefixed with "hk-".
    /// Checks for collisions against existing IDs; extends length if needed.
    pub fn generate_id(existing_ids: &[String]) -> String {
        let ulid_str = ulid::Ulid::new().to_string().to_lowercase();
        let chars: Vec<char> = ulid_str.chars().collect();

        // Try 3 chars, then 4, then 5, etc.
        for len in 3..=ulid_str.len() {
            let suffix: String = chars[chars.len() - len..].iter().collect();
            let id = format!("hk-{}", suffix);
            if !existing_ids.contains(&id) {
                return id;
            }
        }

        // Fallback: full ULID (should never happen with < 100 hooks)
        format!("hk-{}", ulid_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_id_format() {
        let id = Hook::generate_id(&[]);
        assert!(id.starts_with("hk-"));
        // "hk-" + 3 chars = 6 total
        assert_eq!(id.len(), 6);
    }

    #[test]
    fn test_generate_id_avoids_collision() {
        // Generate first ID
        let id1 = Hook::generate_id(&[]);
        // Generate second ID with first as existing — should be different
        let id2 = Hook::generate_id(&[id1.clone()]);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_hook_roundtrip() {
        let hook = Hook {
            id: "hk-abc".to_string(),
            channel: "deploy".to_string(),
            condition: HookCondition::ClaimAvailable {
                pattern: "agent://test-dev".to_string(),
            },
            command: vec!["echo".to_string(), "fired".to_string()],
            cwd: PathBuf::from("/tmp"),
            cooldown_secs: 30,
            last_fired: None,
            created_at: Utc::now(),
            created_by: Some("test-agent".to_string()),
            active: true,
        };

        let json = serde_json::to_string(&hook).unwrap();
        let parsed: Hook = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "hk-abc");
        assert_eq!(parsed.channel, "deploy");
        assert!(parsed.active);
    }

    #[test]
    fn test_hook_firing_roundtrip() {
        let firing = HookFiring {
            ts: Utc::now(),
            hook_id: "hk-abc".to_string(),
            channel: "deploy".to_string(),
            message_id: "01ABCDEF".to_string(),
            condition_result: true,
            executed: true,
            reason: None,
        };

        let json = serde_json::to_string(&firing).unwrap();
        let parsed: HookFiring = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.hook_id, "hk-abc");
        assert!(parsed.executed);
    }

    #[test]
    fn test_condition_serde() {
        let cond = HookCondition::ClaimAvailable {
            pattern: "agent://test".to_string(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        assert!(json.contains("claim_available"));
        let parsed: HookCondition = serde_json::from_str(&json).unwrap();
        match parsed {
            HookCondition::ClaimAvailable { pattern } => {
                assert_eq!(pattern, "agent://test");
            }
        }
    }
}
