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

    /// Optional labels for categorization/filtering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,

    /// Optional file attachments
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,

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
            labels: Vec::new(),
            attachments: Vec::new(),
            meta: None,
        }
    }

    /// Create a new message with metadata.
    pub fn with_meta(mut self, meta: MessageMeta) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Add labels to the message.
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }

    /// Add attachments to the message.
    pub fn with_attachments(mut self, attachments: Vec<Attachment>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Check if message has a specific label.
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    /// Check if message has any of the specified labels.
    pub fn has_any_label(&self, labels: &[String]) -> bool {
        labels.iter().any(|l| self.has_label(l))
    }
}

/// File attachment on a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Display name for the attachment
    pub name: String,

    /// Type of attachment
    #[serde(flatten)]
    pub content: AttachmentContent,
}

/// Content of an attachment - either a file reference or inline content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentContent {
    /// Reference to a file path (relative to project root)
    File { path: String },

    /// Inline text content (for small snippets)
    Inline {
        content: String,
        /// Optional language hint for syntax highlighting
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },

    /// URL reference
    Url { url: String },
}

impl Attachment {
    /// Create a file attachment.
    pub fn file(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: AttachmentContent::File { path: path.into() },
        }
    }

    /// Create an inline content attachment.
    pub fn inline(
        name: impl Into<String>,
        content: impl Into<String>,
        language: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            content: AttachmentContent::Inline {
                content: content.into(),
                language,
            },
        }
    }

    /// Create a URL attachment.
    pub fn url(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: AttachmentContent::Url { url: url.into() },
        }
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

    /// Agent extended an existing claim
    ClaimExtended {
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
    AgentRenamed {
        old_name: String,
    },
    ClaimExpired {
        patterns: Vec<String>,
    },
    HookFired {
        hook_id: String,
        command: Vec<String>,
    },
}

/// Extract @mentions from message body.
fn extract_mentions(body: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let mut chars = body.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '@' {
            let mut name = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_alphanumeric() || next == '_' || next == '-' {
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
        // Test hyphenated agent names (kebab-case)
        assert_eq!(
            extract_mentions("Hey @iron-bear and @swift-falcon"),
            vec!["iron-bear", "swift-falcon"]
        );
        assert_eq!(
            extract_mentions("@multi-word-agent-name test"),
            vec!["multi-word-agent-name"]
        );
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

    #[test]
    fn test_message_with_labels() {
        let msg = Message::new("Agent", "general", "Bug fix ready")
            .with_labels(vec!["bug".to_string(), "ready-for-review".to_string()]);

        assert!(msg.has_label("bug"));
        assert!(msg.has_label("ready-for-review"));
        assert!(!msg.has_label("feature"));

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("labels"));
        assert!(json.contains("bug"));

        // Round-trip
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, vec!["bug", "ready-for-review"]);
    }

    #[test]
    fn test_message_with_attachments() {
        let msg = Message::new("Agent", "general", "See attached").with_attachments(vec![
            Attachment::file("config", "src/config.rs"),
            Attachment::inline("snippet", "fn main() {}", Some("rust".to_string())),
            Attachment::url("docs", "https://example.com/docs"),
        ]);

        assert_eq!(msg.attachments.len(), 3);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("attachments"));
        assert!(json.contains("src/config.rs"));
        assert!(json.contains("fn main()"));

        // Round-trip
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.attachments.len(), 3);
    }

    #[test]
    fn test_has_any_label() {
        let msg = Message::new("Agent", "general", "Test")
            .with_labels(vec!["bug".to_string(), "urgent".to_string()]);

        assert!(msg.has_any_label(&["bug".to_string(), "feature".to_string()]));
        assert!(msg.has_any_label(&["urgent".to_string()]));
        assert!(!msg.has_any_label(&["feature".to_string(), "docs".to_string()]));
        assert!(!msg.has_any_label(&[]));
    }

    #[test]
    fn test_labels_not_serialized_when_empty() {
        let msg = Message::new("Agent", "general", "No labels");
        let json = serde_json::to_string(&msg).unwrap();
        // Empty vecs should not appear in JSON output
        assert!(!json.contains("\"labels\""));
        assert!(!json.contains("\"attachments\""));
    }
}
