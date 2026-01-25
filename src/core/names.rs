//! Agent name generation and validation.
//!
//! Names are kebab-case (IRC style): `swift-falcon`, `blue-castle`, etc.

use rand::seq::IndexedRandom;

const ADJECTIVES: &[&str] = &[
    "blue",
    "green",
    "red",
    "gold",
    "silver",
    "bronze",
    "amber",
    "jade",
    "swift",
    "brave",
    "calm",
    "wild",
    "bold",
    "keen",
    "wise",
    "true",
    "silent",
    "gentle",
    "fierce",
    "noble",
    "ancient",
    "cosmic",
    "crystal",
    "digital",
    "electric",
    "frozen",
    "golden",
    "hidden",
    "iron",
    "jasper",
    "lunar",
    "mystic",
    "northern",
    "onyx",
    "primal",
    "quantum",
    "radiant",
    "sacred",
    "thunder",
    "ultra",
    "velvet",
    "wandering",
    "crimson",
    "violet",
    "scarlet",
    "azure",
    "coral",
    "ivory",
    "obsidian",
    "sterling",
    "twilight",
    "phantom",
    "shadow",
    "ember",
    "frost",
    "storm",
    "dawn",
    "dusk",
    "midnight",
    "stellar",
    "astral",
    "void",
    "nexus",
    "prime",
    "apex",
    "omega",
    "delta",
];

const NOUNS: &[&str] = &[
    "castle", "forest", "river", "mountain", "lake", "storm", "eagle", "wolf", "phoenix", "dragon",
    "falcon", "hawk", "raven", "tiger", "lion", "bear", "anchor", "beacon", "circuit", "depot",
    "engine", "forge", "gateway", "harbor", "index", "junction", "kernel", "lattice", "matrix",
    "nexus", "oracle", "portal", "quartz", "relay", "sentinel", "tower", "umbra", "vertex",
    "warden", "zenith", "fox", "owl", "serpent", "panther", "jaguar", "cobra", "viper", "crane",
    "heron", "otter", "badger", "lynx", "moth", "crow", "finch", "sparrow", "cedar", "oak", "pine",
    "willow", "aspen", "birch", "maple", "fern", "moss", "tide", "wave", "reef",
];

/// Generate a random agent name in kebab-case format (e.g., "swift-falcon").
pub fn generate_name() -> String {
    let mut rng = rand::rng();
    let adjective = ADJECTIVES.choose(&mut rng).unwrap_or(&"swift");
    let noun = NOUNS.choose(&mut rng).unwrap_or(&"agent");
    format!("{}-{}", adjective, noun)
}

/// Validate an agent name.
///
/// Valid names:
/// - Lowercase alphanumeric + hyphens
/// - 1-64 chars
/// - Must start with a letter
/// - No consecutive hyphens
/// - No leading/trailing hyphens
pub fn is_valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }

    // Must start with a letter
    let mut chars = name.chars().peekable();
    if !chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
        return false;
    }

    // No trailing hyphen
    if name.ends_with('-') {
        return false;
    }

    // Check rest: lowercase alphanumeric or hyphen, no consecutive hyphens
    let mut prev_hyphen = false;
    for c in chars {
        if c == '-' {
            if prev_hyphen {
                return false; // Consecutive hyphens
            }
            prev_hyphen = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            prev_hyphen = false;
        } else {
            return false; // Invalid character
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_name() {
        let name = generate_name();
        assert!(!name.is_empty());
        assert!(name.contains('-'), "Name should be kebab-case: {}", name);
        assert!(
            is_valid_name(&name),
            "Generated name should be valid: {}",
            name
        );
    }

    #[test]
    fn test_generate_name_format() {
        // Generate several names and check format
        for _ in 0..10 {
            let name = generate_name();
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
            assert!(name.starts_with(|c: char| c.is_ascii_lowercase()));
        }
    }

    #[test]
    fn test_valid_names() {
        assert!(is_valid_name("swift-falcon"));
        assert!(is_valid_name("blue-castle"));
        assert!(is_valid_name("agent01"));
        assert!(is_valid_name("a"));
        assert!(is_valid_name("my-cool-agent"));
    }

    #[test]
    fn test_invalid_names() {
        assert!(!is_valid_name("")); // Empty
        assert!(!is_valid_name("123agent")); // Starts with number
        assert!(!is_valid_name("-agent")); // Starts with hyphen
        assert!(!is_valid_name("agent-")); // Ends with hyphen
        assert!(!is_valid_name("agent--name")); // Consecutive hyphens
        assert!(!is_valid_name("Agent")); // Uppercase
        assert!(!is_valid_name("my_agent")); // Underscore
        assert!(!is_valid_name("my agent")); // Space
    }
}
