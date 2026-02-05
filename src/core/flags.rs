//! Message flags for controlling hook behavior.
//!
//! Flags are parsed from message bodies and stripped before storage.
//! This allows any message source (CLI, Telegram, TUI) to control hook behavior.
//!
//! ## Supported Flags
//!
//! - `!nohooks` - Suppress all hooks
//! - `!nochanhooks` - Suppress channel hooks only
//! - `!noathooks` - Suppress @mention hooks only
//!
//! Flags are case-insensitive and can appear anywhere in the message body.

use serde::{Deserialize, Serialize};

/// Parsed hook control flags from a message body.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HookFlags {
    /// Suppress all hooks (equivalent to --no-hooks CLI flag)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_hooks: bool,

    /// Suppress channel hooks only
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_chan_hooks: bool,

    /// Suppress @mention hooks only
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_at_hooks: bool,
}

impl HookFlags {
    /// Check if all hooks should be suppressed.
    pub fn suppress_all(&self) -> bool {
        self.no_hooks
    }

    /// Check if channel hooks should be suppressed.
    pub fn suppress_channel_hooks(&self) -> bool {
        self.no_hooks || self.no_chan_hooks
    }

    /// Check if @mention hooks should be suppressed.
    pub fn suppress_mention_hooks(&self) -> bool {
        self.no_hooks || self.no_at_hooks
    }

    /// Check if any flags are set.
    pub fn any_set(&self) -> bool {
        self.no_hooks || self.no_chan_hooks || self.no_at_hooks
    }
}

/// Result of parsing flags from a message body.
#[derive(Debug)]
pub struct ParsedBody {
    /// The message body with flags stripped
    pub body: String,
    /// Parsed hook control flags
    pub flags: HookFlags,
}

/// Parse !flags from a message body and return the cleaned body with flags.
///
/// Flags are case-insensitive and can appear anywhere in the message.
/// Multiple flags can be combined. Flags are stripped from the body
/// and excess whitespace is normalized.
///
/// # Examples
///
/// ```
/// use botbus::core::flags::parse_flags;
///
/// let result = parse_flags("hello world !nohooks");
/// assert_eq!(result.body, "hello world");
/// assert!(result.flags.no_hooks);
///
/// let result = parse_flags("!nochanhooks test !noathooks");
/// assert_eq!(result.body, "test");
/// assert!(result.flags.no_chan_hooks);
/// assert!(result.flags.no_at_hooks);
/// ```
pub fn parse_flags(body: &str) -> ParsedBody {
    let mut flags = HookFlags::default();
    let mut cleaned_parts: Vec<&str> = Vec::new();

    for word in body.split_whitespace() {
        let lower = word.to_lowercase();
        if lower == "!nohooks" {
            flags.no_hooks = true;
        } else if lower == "!nochanhooks" {
            flags.no_chan_hooks = true;
        } else if lower == "!noathooks" {
            flags.no_at_hooks = true;
        } else {
            cleaned_parts.push(word);
        }
    }

    ParsedBody {
        body: cleaned_parts.join(" "),
        flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_no_flags() {
        let result = parse_flags("hello world");
        assert_eq!(result.body, "hello world");
        assert!(!result.flags.no_hooks);
        assert!(!result.flags.no_chan_hooks);
        assert!(!result.flags.no_at_hooks);
    }

    #[test]
    fn test_parse_nohooks() {
        let result = parse_flags("test message !nohooks");
        assert_eq!(result.body, "test message");
        assert!(result.flags.no_hooks);
        assert!(!result.flags.no_chan_hooks);
        assert!(!result.flags.no_at_hooks);
    }

    #[test]
    fn test_parse_nochanhooks() {
        let result = parse_flags("!nochanhooks my message");
        assert_eq!(result.body, "my message");
        assert!(!result.flags.no_hooks);
        assert!(result.flags.no_chan_hooks);
        assert!(!result.flags.no_at_hooks);
    }

    #[test]
    fn test_parse_noathooks() {
        let result = parse_flags("@agent please review !noathooks");
        assert_eq!(result.body, "@agent please review");
        assert!(!result.flags.no_hooks);
        assert!(!result.flags.no_chan_hooks);
        assert!(result.flags.no_at_hooks);
    }

    #[test]
    fn test_parse_multiple_flags() {
        let result = parse_flags("hello !nochanhooks world !noathooks");
        assert_eq!(result.body, "hello world");
        assert!(!result.flags.no_hooks);
        assert!(result.flags.no_chan_hooks);
        assert!(result.flags.no_at_hooks);
    }

    #[test]
    fn test_case_insensitive() {
        let result = parse_flags("test !NoHooks");
        assert_eq!(result.body, "test");
        assert!(result.flags.no_hooks);

        let result = parse_flags("test !NOCHANHOOKS");
        assert_eq!(result.body, "test");
        assert!(result.flags.no_chan_hooks);

        let result = parse_flags("test !NoAtHooks");
        assert_eq!(result.body, "test");
        assert!(result.flags.no_at_hooks);
    }

    #[test]
    fn test_flag_in_middle() {
        let result = parse_flags("start !nohooks end");
        assert_eq!(result.body, "start end");
        assert!(result.flags.no_hooks);
    }

    #[test]
    fn test_suppress_methods() {
        let mut flags = HookFlags::default();

        // No flags set
        assert!(!flags.suppress_all());
        assert!(!flags.suppress_channel_hooks());
        assert!(!flags.suppress_mention_hooks());

        // no_hooks suppresses everything
        flags.no_hooks = true;
        assert!(flags.suppress_all());
        assert!(flags.suppress_channel_hooks());
        assert!(flags.suppress_mention_hooks());

        // no_chan_hooks only suppresses channel hooks
        flags = HookFlags::default();
        flags.no_chan_hooks = true;
        assert!(!flags.suppress_all());
        assert!(flags.suppress_channel_hooks());
        assert!(!flags.suppress_mention_hooks());

        // no_at_hooks only suppresses mention hooks
        flags = HookFlags::default();
        flags.no_at_hooks = true;
        assert!(!flags.suppress_all());
        assert!(!flags.suppress_channel_hooks());
        assert!(flags.suppress_mention_hooks());
    }

    #[test]
    fn test_empty_body_after_flags() {
        let result = parse_flags("!nohooks");
        assert_eq!(result.body, "");
        assert!(result.flags.no_hooks);
    }

    #[test]
    fn test_whitespace_normalization() {
        let result = parse_flags("  hello   !nohooks   world  ");
        assert_eq!(result.body, "hello world");
        assert!(result.flags.no_hooks);
    }

    #[test]
    fn test_any_set() {
        let mut flags = HookFlags::default();
        assert!(!flags.any_set());

        flags.no_hooks = true;
        assert!(flags.any_set());

        flags = HookFlags::default();
        flags.no_chan_hooks = true;
        assert!(flags.any_set());

        flags = HookFlags::default();
        flags.no_at_hooks = true;
        assert!(flags.any_set());
    }
}
