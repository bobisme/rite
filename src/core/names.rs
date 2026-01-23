use rand::seq::IndexedRandom;

const ADJECTIVES: &[&str] = &[
    "Blue",
    "Green",
    "Red",
    "Gold",
    "Silver",
    "Bronze",
    "Amber",
    "Jade",
    "Swift",
    "Brave",
    "Calm",
    "Wild",
    "Bold",
    "Keen",
    "Wise",
    "True",
    "Silent",
    "Gentle",
    "Fierce",
    "Noble",
    "Ancient",
    "Cosmic",
    "Crystal",
    "Digital",
    "Electric",
    "Frozen",
    "Golden",
    "Hidden",
    "Iron",
    "Jasper",
    "Lunar",
    "Mystic",
    "Northern",
    "Onyx",
    "Primal",
    "Quantum",
    "Radiant",
    "Sacred",
    "Thunder",
    "Ultra",
    "Velvet",
    "Wandering",
    "Xenon",
    "Yielding",
    "Zealous",
];

const NOUNS: &[&str] = &[
    "Castle", "Forest", "River", "Mountain", "Lake", "Storm", "Eagle", "Wolf", "Phoenix", "Dragon",
    "Falcon", "Hawk", "Raven", "Tiger", "Lion", "Bear", "Anchor", "Beacon", "Circuit", "Depot",
    "Engine", "Forge", "Gateway", "Harbor", "Index", "Junction", "Kernel", "Lattice", "Matrix",
    "Nexus", "Oracle", "Portal", "Quartz", "Relay", "Sentinel", "Tower", "Umbra", "Vertex",
    "Warden", "Zenith",
];

/// Generate a random agent name in PascalCase format (e.g., "BlueCastle").
pub fn generate_agent_name() -> String {
    let mut rng = rand::rng();
    let adjective = ADJECTIVES.choose(&mut rng).unwrap_or(&"Swift");
    let noun = NOUNS.choose(&mut rng).unwrap_or(&"Agent");
    format!("{}{}", adjective, noun)
}

/// Generate a unique agent name, appending a number if the base name exists.
pub fn generate_unique_name<F>(exists: F) -> String
where
    F: Fn(&str) -> bool,
{
    let base_name = generate_agent_name();

    if !exists(&base_name) {
        return base_name;
    }

    // Try appending numbers 01-99
    for i in 1..100 {
        let numbered_name = format!("{}{:02}", base_name, i);
        if !exists(&numbered_name) {
            return numbered_name;
        }
    }

    // Fallback: generate a completely new name
    for _ in 0..100 {
        let new_name = generate_agent_name();
        if !exists(&new_name) {
            return new_name;
        }
    }

    // Ultimate fallback with timestamp
    format!("Agent{}", chrono::Utc::now().timestamp_millis())
}

/// Validate an agent name.
pub fn is_valid_agent_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }

    // Must start with a letter
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return false;
    }

    // Rest must be alphanumeric or underscore
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_agent_name() {
        let name = generate_agent_name();
        assert!(!name.is_empty());
        assert!(is_valid_agent_name(&name));
    }

    #[test]
    fn test_generate_unique_name() {
        let existing = vec!["BlueCastle".to_string()];
        let name = generate_unique_name(|n| existing.contains(&n.to_string()));
        assert!(is_valid_agent_name(&name));
    }

    #[test]
    fn test_valid_agent_names() {
        assert!(is_valid_agent_name("BlueCastle"));
        assert!(is_valid_agent_name("Agent01"));
        assert!(is_valid_agent_name("My_Agent"));
        assert!(!is_valid_agent_name(""));
        assert!(!is_valid_agent_name("123Agent"));
        assert!(!is_valid_agent_name("Agent-Name"));
    }
}
