//! Agent identity resolution.
//!
//! Identity is determined by (in order of precedence):
//! 1. Explicit --agent flag
//! 2. RITE_AGENT environment variable
//! 3. AGENT environment variable (generic fallback)
//! 4. USER environment variable (only when stdout is a TTY — human convenience)
//!
//! Rite is stateless about identity - it trusts whatever name is provided.
//! The orchestrator/user is responsible for persisting identity across sessions.

use anyhow::{Result, anyhow};
use std::env;
use std::io::IsTerminal;

/// Environment variable name for agent identity.
pub const AGENT_ENV_VAR: &str = "RITE_AGENT";

/// Resolve the current agent identity.
///
/// Checks in order:
/// 1. Explicit agent name (from --agent flag)
/// 2. RITE_AGENT environment variable
/// 3. AGENT environment variable (generic fallback)
/// 4. USER environment variable (only when stdout is a TTY)
///
/// Returns None if no identity is configured.
pub fn resolve_agent(explicit: Option<&str>) -> Option<String> {
    // 1. Explicit flag takes precedence
    if let Some(name) = explicit {
        return Some(name.to_string());
    }

    // 2. RITE_AGENT environment variable
    if let Ok(name) = env::var(AGENT_ENV_VAR)
        && !name.is_empty()
    {
        return Some(name);
    }

    // 3. AGENT environment variable (generic fallback)
    if let Ok(name) = env::var("AGENT")
        && !name.is_empty()
    {
        return Some(name);
    }

    // 4. USER env var — only in interactive TTY sessions (human convenience)
    if std::io::stdout().is_terminal()
        && let Ok(name) = env::var("USER")
        && !name.is_empty()
    {
        return Some(name);
    }

    None
}

/// Require an agent identity, returning an error with helpful message if not set.
pub fn require_agent(explicit: Option<&str>) -> Result<String> {
    resolve_agent(explicit).ok_or_else(|| {
        anyhow!(
            "RITE_AGENT environment variable not set.\n\n\
             Set your identity:\n  \
             export RITE_AGENT=$(rite generate-name)\n\n\
             Or choose your own name (kebab-case preferred):\n  \
             export RITE_AGENT=my-agent-name"
        )
    })
}

/// Check if an agent identity is available.
pub fn has_identity(explicit: Option<&str>) -> bool {
    resolve_agent(explicit).is_some()
}

/// Format the export command for shell usage.
pub fn format_export(agent_name: &str) -> String {
    format!("export {}={}", AGENT_ENV_VAR, agent_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    #[test]
    #[serial]
    fn test_explicit_takes_precedence() {
        // SAFETY: Test isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "env-agent");
        }

        // Explicit should win
        let result = resolve_agent(Some("explicit-agent"));
        assert_eq!(result, Some("explicit-agent".to_string()));

        // Clean up
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    #[serial]
    fn test_env_var_used_when_no_explicit() {
        // SAFETY: Test isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "env-agent");
        }

        let result = resolve_agent(None);
        assert_eq!(result, Some("env-agent".to_string()));

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    #[serial]
    fn test_agent_env_fallback() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
            env::set_var("AGENT", "generic-agent");
        }

        let result = resolve_agent(None);
        assert_eq!(result, Some("generic-agent".to_string()));

        unsafe {
            env::remove_var("AGENT");
        }
    }

    #[test]
    #[serial]
    fn test_rite_agent_takes_precedence_over_agent() {
        // SAFETY: Test isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "rite-agent");
            env::set_var("AGENT", "generic-agent");
        }

        let result = resolve_agent(None);
        assert_eq!(result, Some("rite-agent".to_string()));

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
            env::remove_var("AGENT");
        }
    }

    #[test]
    #[serial]
    fn test_no_identity() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
            env::remove_var("AGENT");
        }

        let result = resolve_agent(None);
        assert!(result.is_none());
    }

    #[test]
    #[serial]
    fn test_require_agent_error() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
            env::remove_var("AGENT");
        }

        let result = require_agent(None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("RITE_AGENT"));
        assert!(err.contains("generate-name"));
    }

    #[test]
    fn test_format_export() {
        let export = format_export("my-agent");
        assert_eq!(export, "export RITE_AGENT=my-agent");
    }
}
