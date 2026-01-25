//! Output formatting utilities for different output formats.
//!
//! Supports:
//! - Text: Human-readable colored output (default)
//! - JSON: Standard machine-readable format
//! - TOON: Text-Only Object Notation, optimized for AI agents
//!
//! TOON format is a simple, token-efficient format:
//! ```text
//! key: value
//! nested.key: value
//! list.0: first item
//! list.1: second item
//! ---
//! ```

use serde::Serialize;

use super::OutputFormat;

/// Format a serializable value according to the output format.
pub fn format_output<T: Serialize>(value: &T, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => {
            // Text format should be handled by the caller with custom formatting
            // This is a fallback that uses debug representation
            format!("{:?}", serde_json::to_value(value).unwrap_or_default())
        }
        OutputFormat::Json => serde_json::to_string_pretty(value).unwrap_or_default(),
        OutputFormat::Toon => to_toon(value),
    }
}

/// Convert a serializable value to TOON format.
///
/// TOON (Text-Only Object Notation) is designed for AI agents:
/// - Simple key: value pairs
/// - Nested keys use dot notation
/// - Arrays use numeric indices
/// - Records separated by ---
pub fn to_toon<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_value(value).unwrap_or_default();
    let mut lines = Vec::new();
    flatten_to_toon(&json, "", &mut lines);
    lines.join("\n")
}

/// Convert a list of items to TOON format with record separators.
pub fn to_toon_list<T: Serialize>(items: &[T]) -> String {
    items
        .iter()
        .map(|item| to_toon(item))
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn flatten_to_toon(value: &serde_json::Value, prefix: &str, lines: &mut Vec<String>) {
    match value {
        serde_json::Value::Null => {
            if !prefix.is_empty() {
                lines.push(format!("{}: null", prefix));
            }
        }
        serde_json::Value::Bool(b) => {
            if !prefix.is_empty() {
                lines.push(format!("{}: {}", prefix, b));
            }
        }
        serde_json::Value::Number(n) => {
            if !prefix.is_empty() {
                lines.push(format!("{}: {}", prefix, n));
            }
        }
        serde_json::Value::String(s) => {
            if !prefix.is_empty() {
                // Handle multi-line strings by indenting continuation lines
                if s.contains('\n') {
                    let escaped = s.replace('\n', "\n  ");
                    lines.push(format!("{}: {}", prefix, escaped));
                } else {
                    lines.push(format!("{}: {}", prefix, s));
                }
            }
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                if !prefix.is_empty() {
                    lines.push(format!("{}: []", prefix));
                }
            } else {
                for (i, item) in arr.iter().enumerate() {
                    let key = if prefix.is_empty() {
                        i.to_string()
                    } else {
                        format!("{}.{}", prefix, i)
                    };
                    flatten_to_toon(item, &key, lines);
                }
            }
        }
        serde_json::Value::Object(obj) => {
            if obj.is_empty() {
                if !prefix.is_empty() {
                    lines.push(format!("{}: {{}}", prefix));
                }
            } else {
                for (k, v) in obj {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{}.{}", prefix, k)
                    };
                    flatten_to_toon(v, &key, lines);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_object() {
        let value = json!({
            "name": "Alice",
            "age": 30
        });
        let toon = to_toon(&value);
        assert!(toon.contains("name: Alice"));
        assert!(toon.contains("age: 30"));
    }

    #[test]
    fn test_nested_object() {
        let value = json!({
            "user": {
                "name": "Bob",
                "email": "bob@example.com"
            }
        });
        let toon = to_toon(&value);
        assert!(toon.contains("user.name: Bob"));
        assert!(toon.contains("user.email: bob@example.com"));
    }

    #[test]
    fn test_array() {
        let value = json!({
            "items": ["a", "b", "c"]
        });
        let toon = to_toon(&value);
        assert!(toon.contains("items.0: a"));
        assert!(toon.contains("items.1: b"));
        assert!(toon.contains("items.2: c"));
    }

    #[test]
    fn test_toon_list() {
        #[derive(Serialize)]
        struct Item {
            id: u32,
            name: String,
        }

        let items = vec![
            Item {
                id: 1,
                name: "First".to_string(),
            },
            Item {
                id: 2,
                name: "Second".to_string(),
            },
        ];

        let toon = to_toon_list(&items);
        assert!(toon.contains("id: 1"));
        assert!(toon.contains("name: First"));
        assert!(toon.contains("---"));
        assert!(toon.contains("id: 2"));
        assert!(toon.contains("name: Second"));
    }

    #[test]
    fn test_empty_values() {
        let value = json!({
            "empty_array": [],
            "empty_object": {},
            "null_value": null
        });
        let toon = to_toon(&value);
        assert!(toon.contains("empty_array: []"));
        assert!(toon.contains("empty_object: {}"));
        assert!(toon.contains("null_value: null"));
    }
}
