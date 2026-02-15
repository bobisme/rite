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

    /// How to release the claim after hook fires.
    /// None for hooks created before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_release: Option<ClaimRelease>,

    /// Explicit claim pattern for mention hooks (from --claim).
    /// ClaimAvailable hooks use their condition pattern instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_pattern: Option<String>,

    /// Agent that should own the claim (default: message sender).
    /// Useful when spawning an agent that needs to refresh its own claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_owner: Option<String>,

    /// Priority for hook execution (lower runs first, Unix convention).
    /// Default: 0
    #[serde(default)]
    pub priority: i32,

    /// Only fire this hook if the message contains the specified !flag.
    /// E.g., require_flag = "dev" means the hook only fires on messages containing "!dev".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_flag: Option<String>,

    /// Whether this hook is active
    pub active: bool,

    /// Optional description for identification/deduplication
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// How to release the claim acquired when a hook fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaimRelease {
    /// Hold claim for a fixed duration (seconds).
    Ttl { secs: u64 },
    /// Release claim when the spawned command exits.
    OnExit,
}

/// Condition that must be met for a hook to fire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookCondition {
    /// Fire when no active claim holds the given pattern.
    ClaimAvailable { pattern: String },
    /// Fire when a message contains a specific @mention.
    MentionReceived { agent: String },
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

/// Format a command as a shell would display it, quoting args that need it.
pub fn shell_display(cmd: &[String]) -> String {
    cmd.iter()
        .map(|arg| {
            if arg.is_empty() {
                "''".to_string()
            } else if arg
                .chars()
                .any(|c| " \t\n\"'\\$`!#&|;(){}[]<>?*~".contains(c))
            {
                // Wrap in single quotes, escaping embedded single quotes
                format!("'{}'", arg.replace('\'', "'\\''"))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
            claim_release: Some(ClaimRelease::OnExit),
            claim_pattern: None,
            claim_owner: None,
            priority: 0,
            require_flag: None,
            active: true,
            description: None,
        };

        let json = serde_json::to_string(&hook).unwrap();
        let parsed: Hook = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "hk-abc");
        assert_eq!(parsed.channel, "deploy");
        assert!(parsed.active);
        assert!(parsed.claim_release.is_some());
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
    fn test_shell_display() {
        // Simple command
        assert_eq!(
            shell_display(&["echo".into(), "hello".into()]),
            "echo hello"
        );
        // Arg with spaces gets quoted
        assert_eq!(
            shell_display(&["echo".into(), "hello world".into()]),
            "echo 'hello world'"
        );
        // Arg with single quote gets escaped
        assert_eq!(
            shell_display(&["echo".into(), "it's".into()]),
            "echo 'it'\\''s'"
        );
        // Empty arg
        assert_eq!(shell_display(&["echo".into(), "".into()]), "echo ''");
        // No args
        assert_eq!(shell_display(&[]), "");
    }

    #[test]
    fn test_claim_release_serde() {
        let ttl = ClaimRelease::Ttl { secs: 300 };
        let json = serde_json::to_string(&ttl).unwrap();
        assert!(json.contains("\"type\":\"ttl\""));
        let parsed: ClaimRelease = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ClaimRelease::Ttl { secs: 300 }));

        let on_exit = ClaimRelease::OnExit;
        let json = serde_json::to_string(&on_exit).unwrap();
        assert!(json.contains("\"type\":\"on_exit\""));
        let parsed: ClaimRelease = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, ClaimRelease::OnExit));
    }

    #[test]
    fn test_hook_backward_compat_no_claim_release() {
        // Simulate old hook JSON without claim_release, created_by, priority, or description fields
        let json = r#"{"id":"hk-old","channel":"test","condition":{"type":"claim_available","pattern":"x"},"command":["echo"],"cwd":"/tmp","cooldown_secs":30,"created_at":"2025-01-01T00:00:00Z","active":true}"#;
        let hook: Hook = serde_json::from_str(json).unwrap();
        assert!(hook.claim_release.is_none());
        assert!(hook.created_by.is_none());
        assert_eq!(hook.priority, 0);
        assert!(hook.require_flag.is_none());
        assert!(hook.description.is_none());
    }

    #[test]
    fn test_description_roundtrip() {
        let hook = Hook {
            id: "hk-desc".to_string(),
            channel: "test".to_string(),
            condition: HookCondition::MentionReceived {
                agent: "test-agent".to_string(),
            },
            command: vec!["echo".to_string()],
            cwd: PathBuf::from("/tmp"),
            cooldown_secs: 30,
            last_fired: None,
            created_at: Utc::now(),
            created_by: None,
            claim_release: None,
            claim_pattern: None,
            claim_owner: None,
            priority: 0,
            require_flag: None,
            active: true,
            description: Some("botbox:respond:general".to_string()),
        };

        let json = serde_json::to_string(&hook).unwrap();
        assert!(json.contains("description"));
        assert!(json.contains("botbox:respond:general"));
        let parsed: Hook = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.description,
            Some("botbox:respond:general".to_string())
        );
    }

    #[test]
    fn test_description_omitted_when_none() {
        let hook = Hook {
            id: "hk-nodesc".to_string(),
            channel: "test".to_string(),
            condition: HookCondition::MentionReceived {
                agent: "test-agent".to_string(),
            },
            command: vec!["echo".to_string()],
            cwd: PathBuf::from("/tmp"),
            cooldown_secs: 30,
            last_fired: None,
            created_at: Utc::now(),
            created_by: None,
            claim_release: None,
            claim_pattern: None,
            claim_owner: None,
            priority: 0,
            require_flag: None,
            active: true,
            description: None,
        };

        let json = serde_json::to_string(&hook).unwrap();
        assert!(!json.contains("description"));
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
            HookCondition::MentionReceived { .. } => {
                panic!("Expected ClaimAvailable, got MentionReceived");
            }
        }
    }

    #[test]
    fn test_mention_condition_serde() {
        let cond = HookCondition::MentionReceived {
            agent: "security-reviewer".to_string(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        assert!(json.contains("mention_received"));
        assert!(json.contains("security-reviewer"));
        let parsed: HookCondition = serde_json::from_str(&json).unwrap();
        match parsed {
            HookCondition::MentionReceived { agent } => {
                assert_eq!(agent, "security-reviewer");
            }
            HookCondition::ClaimAvailable { .. } => {
                panic!("Expected MentionReceived, got ClaimAvailable");
            }
        }
    }

    #[test]
    fn test_require_flag_roundtrip() {
        let hook = Hook {
            id: "hk-flg".to_string(),
            channel: "deploy".to_string(),
            condition: HookCondition::ClaimAvailable {
                pattern: "agent://test-dev".to_string(),
            },
            command: vec!["echo".to_string(), "fired".to_string()],
            cwd: PathBuf::from("/tmp"),
            cooldown_secs: 30,
            last_fired: None,
            created_at: Utc::now(),
            created_by: None,
            claim_release: Some(ClaimRelease::OnExit),
            claim_pattern: None,
            claim_owner: None,
            priority: 0,
            require_flag: Some("dev".to_string()),
            active: true,
            description: None,
        };

        let json = serde_json::to_string(&hook).unwrap();
        assert!(json.contains("require_flag"));
        assert!(json.contains("dev"));
        let parsed: Hook = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.require_flag, Some("dev".to_string()));
    }

    #[test]
    fn test_require_flag_omitted_when_none() {
        let hook = Hook {
            id: "hk-nfl".to_string(),
            channel: "deploy".to_string(),
            condition: HookCondition::ClaimAvailable {
                pattern: "agent://test-dev".to_string(),
            },
            command: vec!["echo".to_string()],
            cwd: PathBuf::from("/tmp"),
            cooldown_secs: 30,
            last_fired: None,
            created_at: Utc::now(),
            created_by: None,
            claim_release: None,
            claim_pattern: None,
            claim_owner: None,
            priority: 0,
            require_flag: None,
            active: true,
            description: None,
        };

        let json = serde_json::to_string(&hook).unwrap();
        assert!(!json.contains("require_flag"));
    }
}
