/// Validate a channel name.
///
/// Channel naming rules:
/// - Regular channels: lowercase alphanumeric + hyphens, 1-64 chars
///   - Examples: `general`, `backend`, `webapp-api`, `project-topic`
///   - Use hyphens to separate words: `my-channel` not `my.channel`
/// - DM channels: `_dm_{agent1}_{agent2}` where names are sorted alphabetically
/// - Reserved prefix: `_` for system channels
pub fn is_valid_channel_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }

    // DM channels have special format
    if name.starts_with("_dm_") {
        return is_valid_dm_channel(name);
    }

    // System channels start with _ but aren't DMs
    if name.starts_with('_') {
        // For now, only allow _dm_ prefix for system channels
        return false;
    }

    // Regular channels: lowercase alphanumeric + hyphens
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// Validate a DM channel name format.
fn is_valid_dm_channel(name: &str) -> bool {
    let parts: Vec<&str> = name.strip_prefix("_dm_").unwrap_or("").split('_').collect();
    if parts.len() != 2 {
        return false;
    }

    let (a, b) = (parts[0], parts[1]);

    // Both must be valid agent names (simplified check)
    if a.is_empty() || b.is_empty() {
        return false;
    }

    // Names must be sorted
    a < b
}

/// Create a DM channel name from two agent names.
/// The names are sorted alphabetically to ensure consistency.
pub fn dm_channel_name(agent1: &str, agent2: &str) -> String {
    let (a, b) = if agent1 < agent2 {
        (agent1, agent2)
    } else {
        (agent2, agent1)
    };
    format!("_dm_{}_{}", a, b)
}

/// Check if a channel name represents a DM.
pub fn is_dm_channel(name: &str) -> bool {
    name.starts_with("_dm_")
}

/// Check if input looks like a DM target (starts with @).
pub fn is_dm_target(target: &str) -> bool {
    target.starts_with('@')
}

/// Resolve a channel argument to the actual channel name.
/// - `@agent` → resolved to DM channel name using current agent
/// - `#general` → strip # prefix and return as `general`
/// - `general` → returned as-is
/// - `_dm_a_b` → returned as-is
pub fn resolve_channel(channel: &str, current_agent: Option<&str>) -> Option<String> {
    // Strip # prefix if present (common user pattern)
    let channel = channel.strip_prefix('#').unwrap_or(channel);

    if channel.starts_with('@') {
        // DM target - need current agent to resolve
        let other = channel.strip_prefix('@')?;
        let agent = current_agent?;
        Some(dm_channel_name(agent, other))
    } else {
        // Regular channel or already-resolved DM channel
        Some(channel.to_string())
    }
}

/// Extract agent names from a DM channel name.
pub fn dm_agents(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.strip_prefix("_dm_")?.split('_').collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_channel_names() {
        assert!(is_valid_channel_name("general"));
        assert!(is_valid_channel_name("backend"));
        assert!(is_valid_channel_name("my-channel"));
        assert!(is_valid_channel_name("channel123"));

        assert!(!is_valid_channel_name(""));
        assert!(!is_valid_channel_name("UPPERCASE"));
        assert!(!is_valid_channel_name("-starts-with-dash"));
        assert!(!is_valid_channel_name("ends-with-dash-"));
        assert!(!is_valid_channel_name("has space"));
        assert!(!is_valid_channel_name("has_underscore"));
    }

    #[test]
    fn test_dm_channel() {
        assert_eq!(dm_channel_name("Alice", "Bob"), "_dm_Alice_Bob");
        assert_eq!(dm_channel_name("Bob", "Alice"), "_dm_Alice_Bob");

        assert!(is_valid_channel_name("_dm_Alice_Bob"));
        assert!(is_dm_channel("_dm_Alice_Bob"));
        assert!(!is_dm_channel("general"));
    }

    #[test]
    fn test_dm_agents() {
        assert_eq!(
            dm_agents("_dm_Alice_Bob"),
            Some(("Alice".to_string(), "Bob".to_string()))
        );
        assert_eq!(dm_agents("general"), None);
    }

    #[test]
    fn test_resolve_channel() {
        // Regular channel
        assert_eq!(
            resolve_channel("general", Some("alice")),
            Some("general".to_string())
        );

        // Channel with # prefix - strip it
        assert_eq!(
            resolve_channel("#general", Some("alice")),
            Some("general".to_string())
        );

        assert_eq!(
            resolve_channel("#backend", Some("alice")),
            Some("backend".to_string())
        );

        // DM target resolves to canonical DM channel name
        assert_eq!(
            resolve_channel("@bob", Some("alice")),
            Some("_dm_alice_bob".to_string())
        );

        // Order is normalized
        assert_eq!(
            resolve_channel("@alice", Some("bob")),
            Some("_dm_alice_bob".to_string())
        );

        // Without current agent, DM target can't resolve
        assert_eq!(resolve_channel("@bob", None), None);

        // Already-resolved DM channel passes through
        assert_eq!(
            resolve_channel("_dm_alice_bob", Some("alice")),
            Some("_dm_alice_bob".to_string())
        );
    }
}
