//! Output formatting utilities for different output formats.
//!
//! Supports:
//! - Pretty: Human-readable colored output
//! - Text: Concise text for AI agents
//! - JSON: Standard machine-readable format

use serde::Serialize;

use super::OutputFormat;

/// Format a serializable value according to the output format.
pub fn format_output<T: Serialize>(value: &T, format: OutputFormat) -> String {
    match format {
        OutputFormat::Pretty | OutputFormat::Text => {
            // Text/Pretty formats should be handled by the caller with custom formatting
            // This is a fallback that uses debug representation
            format!("{:?}", serde_json::to_value(value).unwrap_or_default())
        }
        OutputFormat::Json => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}
