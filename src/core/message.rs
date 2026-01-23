use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The fundamental unit of communication in BotBus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Timestamp when the message was created
    pub ts: DateTime<Utc>,

    /// Unique identifier (ULID for sortability without coordination)
    pub id: Ulid,

    /// Name of the sending agent
    pub agent: String,

    /// Channel name, or "_dm_{agent1}_{agent2}" for DMs (names sorted)
    pub channel: String,

    /// Message content (markdown supported)
    pub body: String,

    /// Extracted @mentions for potential notifications
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,

    /// Optional structured metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<MessageMeta>,
}

impl Message {
    /// Create a new message with the current timestamp and a fresh ULID.
    pub fn new(
        agent: impl Into<String>,
        channel: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        let body = body.into();
        let mentions = extract_mentions(&body);

        Self {
            ts: Utc::now(),
            id: Ulid::new(),
            agent: agent.into(),
            channel: channel.into(),
            body,
            mentions,
            meta: None,
        }
    }

    /// Create a new message with metadata.
    pub fn with_meta(mut self, meta: MessageMeta) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// Structured metadata for special message types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageMeta {
    /// Agent claimed files for editing
    Claim {
        patterns: Vec<String>,
        ttl_secs: u64,
    },

    /// Agent released file claims
    Release { patterns: Vec<String> },

    /// System event (agent joined, etc.)
    System { event: SystemEvent },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemEvent {
    AgentRegistered,
    AgentRenamed { old_name: String },
    ClaimExpired { patterns: Vec<String> },
}

/// Extract @mentions from message body.
fn extract_mentions(body: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let mut chars = body.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '@' {
            let mut name = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_alphanumeric() || next == '_' {
                    name.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if !name.is_empty() {
                mentions.push(name);
            }
        }
    }

    mentions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_roundtrip() {
        let msg = Message::new("TestAgent", "general", "Hello, world!");

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();

        assert_eq!(msg.id, parsed.id);
        assert_eq!(msg.body, parsed.body);
        assert_eq!(msg.agent, parsed.agent);
        assert_eq!(msg.channel, parsed.channel);
    }

    #[test]
    fn test_extract_mentions() {
        assert_eq!(
            extract_mentions("Hello @Alice and @Bob"),
            vec!["Alice", "Bob"]
        );
        assert_eq!(extract_mentions("No mentions here"), Vec::<String>::new());
        assert_eq!(extract_mentions("@SingleMention"), vec!["SingleMention"]);
        assert_eq!(extract_mentions("Email test@example.com"), vec!["example"]);
    }

    #[test]
    fn test_message_with_meta() {
        let msg =
            Message::new("Agent", "general", "Claiming files").with_meta(MessageMeta::Claim {
                patterns: vec!["src/**/*.rs".to_string()],
                ttl_secs: 3600,
            });

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("claim"));
        assert!(json.contains("src/**/*.rs"));
    }
}
