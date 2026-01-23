/// Validate a channel name.
///
/// Channel naming rules:
/// - Regular channels: lowercase alphanumeric + hyphens, 1-64 chars
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
}
