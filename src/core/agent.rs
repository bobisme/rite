use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Registered agent identity within a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Timestamp of registration
    pub ts: DateTime<Utc>,

    /// Unique agent name within this project
    pub name: String,

    /// Optional description or identifier (e.g., "Claude Sonnet 3.5")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Registration event type
    pub event: AgentEvent,
}

impl Agent {
    /// Create a new agent registration record.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            ts: Utc::now(),
            name: name.into(),
            description: None,
            event: AgentEvent::Registered,
        }
    }

    /// Create an agent with a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Create a rename event record.
    pub fn renamed(new_name: impl Into<String>, old_name: impl Into<String>) -> Self {
        Self {
            ts: Utc::now(),
            name: new_name.into(),
            description: None,
            event: AgentEvent::Renamed {
                old_name: old_name.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    Registered,
    Renamed { old_name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_roundtrip() {
        let agent = Agent::new("BlueCastle").with_description("Claude Sonnet 3.5");

        let json = serde_json::to_string(&agent).unwrap();
        let parsed: Agent = serde_json::from_str(&json).unwrap();

        assert_eq!(agent.name, parsed.name);
        assert_eq!(agent.description, parsed.description);
    }

    #[test]
    fn test_agent_renamed() {
        let agent = Agent::renamed("NewName", "OldName");

        let json = serde_json::to_string(&agent).unwrap();
        assert!(json.contains("renamed"));
        assert!(json.contains("OldName"));
    }
}
