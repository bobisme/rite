//! Agent identity resolution.
//!
//! Identity is determined by (in order of precedence):
//! 1. Explicit --agent flag
//! 2. BOTBUS_AGENT environment variable
//!
//! BotBus is stateless about identity - it trusts whatever name is provided.
//! The orchestrator/user is responsible for persisting identity across sessions.

use anyhow::{Result, anyhow};
use std::env;

/// Environment variable name for agent identity.
pub const AGENT_ENV_VAR: &str = "BOTBUS_AGENT";

/// Resolve the current agent identity.
///
/// Checks in order:
/// 1. Explicit agent name (from --agent flag)
/// 2. BOTBUS_AGENT environment variable
///
/// Returns None if no identity is configured.
pub fn resolve_agent(explicit: Option<&str>) -> Option<String> {
    // 1. Explicit flag takes precedence
    if let Some(name) = explicit {
        return Some(name.to_string());
    }

    // 2. Environment variable
    if let Ok(name) = env::var(AGENT_ENV_VAR)
        && !name.is_empty() {
            return Some(name);
        }

    None
}

/// Require an agent identity, returning an error with helpful message if not set.
pub fn require_agent(explicit: Option<&str>) -> Result<String> {
    resolve_agent(explicit).ok_or_else(|| {
        anyhow!(
            "BOTBUS_AGENT environment variable not set.\n\n\
             Set your identity:\n  \
             export BOTBUS_AGENT=$(botbus generate-name)\n\n\
             Or choose your own name (kebab-case preferred):\n  \
             export BOTBUS_AGENT=my-agent-name"
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
    use std::env;

    #[test]
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
    fn test_no_identity() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = resolve_agent(None);
        assert!(result.is_none());
    }

    #[test]
    fn test_require_agent_error() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = require_agent(None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("BOTBUS_AGENT"));
        assert!(err.contains("generate-name"));
    }

    #[test]
    fn test_format_export() {
        let export = format_export("my-agent");
        assert_eq!(export, "export BOTBUS_AGENT=my-agent");
    }
}
