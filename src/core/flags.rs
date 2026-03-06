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

    /// Custom !flags found in the message (lowercased, without the ! prefix).
    /// Used by hooks with require_flag to gate on specific flags like !dev.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_flags: Vec<String>,
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

    /// Check if a specific custom flag is present (case-insensitive).
    pub fn has_custom_flag(&self, flag: &str) -> bool {
        let lower = flag.to_lowercase();
        self.custom_flags.iter().any(|f| f == &lower)
    }
}

/// Result of parsing flags from a message body.
#[derive(Debug)]
pub struct ParsedBody {
    /// The message body with flags stripped
    pub body: String,
    /// Parsed hook control flags
    pub flags: HookFlags,
    /// Custom !flags found in the message (lowercased, without the ! prefix).
    /// These are any !word tokens that aren't known suppression flags.
    pub custom_flags: Vec<String>,
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
/// use rite::core::flags::parse_flags;
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
    let mut custom_flags: Vec<String> = Vec::new();

    for word in body.split_whitespace() {
        let lower = word.to_lowercase();
        if lower == "!nohooks" {
            flags.no_hooks = true;
        } else if lower == "!nochanhooks" {
            flags.no_chan_hooks = true;
        } else if lower == "!noathooks" {
            flags.no_at_hooks = true;
        } else if lower.starts_with('!')
            && lower.len() > 1
            && lower[1..]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            custom_flags.push(lower[1..].to_string());
        } else {
            cleaned_parts.push(word);
        }
    }

    flags.custom_flags = custom_flags.clone();

    ParsedBody {
        body: cleaned_parts.join(" "),
        flags,
        custom_flags,
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

    #[test]
    fn test_custom_flags_parsed() {
        let result = parse_flags("hello !dev world");
        assert_eq!(result.body, "hello world");
        assert_eq!(result.custom_flags, vec!["dev"]);
        assert!(result.flags.has_custom_flag("dev"));
    }

    #[test]
    fn test_custom_flags_case_insensitive() {
        let result = parse_flags("!Dev message");
        assert_eq!(result.body, "message");
        assert!(result.flags.has_custom_flag("dev"));
        assert!(result.flags.has_custom_flag("Dev"));
    }

    #[test]
    fn test_multiple_custom_flags() {
        let result = parse_flags("!dev !urgent do something");
        assert_eq!(result.body, "do something");
        assert_eq!(result.custom_flags.len(), 2);
        assert!(result.flags.has_custom_flag("dev"));
        assert!(result.flags.has_custom_flag("urgent"));
    }

    #[test]
    fn test_custom_flags_with_suppression_flags() {
        let result = parse_flags("!dev !nohooks message");
        assert_eq!(result.body, "message");
        assert!(result.flags.no_hooks);
        assert!(result.flags.has_custom_flag("dev"));
        // !nohooks is a suppression flag, not a custom flag
        assert!(!result.flags.has_custom_flag("nohooks"));
    }

    #[test]
    fn test_custom_flags_with_hyphens_underscores() {
        let result = parse_flags("!my-flag !other_flag text");
        assert_eq!(result.body, "text");
        assert!(result.flags.has_custom_flag("my-flag"));
        assert!(result.flags.has_custom_flag("other_flag"));
    }

    #[test]
    fn test_exclamation_in_text_not_flag() {
        // Exclamation followed by space or at end of word isn't a flag
        let result = parse_flags("hello! world");
        assert_eq!(result.body, "hello! world");
        assert!(result.custom_flags.is_empty());
    }

    #[test]
    fn test_has_custom_flag_not_present() {
        let flags = HookFlags::default();
        assert!(!flags.has_custom_flag("dev"));
    }
}
