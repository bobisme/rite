//! Agent identity resolution.
//!
//! Identity is determined by (in order of precedence):
//! 1. Explicit --agent flag
//! 2. BOTBUS_AGENT environment variable
//! 3. Legacy: current_agent in state.json (for backwards compatibility)

use std::env;
use std::path::Path;

use crate::core::project::state_path;
use crate::storage::state::ProjectState;

/// Environment variable name for agent identity.
pub const AGENT_ENV_VAR: &str = "BOTBUS_AGENT";

/// Resolve the current agent identity.
///
/// Checks in order:
/// 1. Explicit agent name (from --agent flag)
/// 2. BOTBUS_AGENT environment variable
/// 3. Legacy state file (for backwards compatibility)
///
/// Returns None if no identity is configured.
pub fn resolve_agent(explicit: Option<&str>, project_root: &Path) -> Option<String> {
    // 1. Explicit flag takes precedence
    if let Some(name) = explicit {
        return Some(name.to_string());
    }

    // 2. Environment variable
    if let Ok(name) = env::var(AGENT_ENV_VAR) {
        if !name.is_empty() {
            return Some(name);
        }
    }

    // 3. Legacy: state file (for backwards compatibility)
    let state = ProjectState::new(state_path(project_root));
    state.current_agent().ok().flatten()
}

/// Check if an agent identity is available.
pub fn has_identity(explicit: Option<&str>, project_root: &Path) -> bool {
    resolve_agent(explicit, project_root).is_some()
}

/// Format the export command for shell usage.
pub fn format_export(agent_name: &str) -> String {
    format!("export {}={}", AGENT_ENV_VAR, agent_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init;
    use std::env;
    use tempfile::TempDir;

    fn setup_project() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_explicit_takes_precedence() {
        let temp = setup_project();

        // Set env var
        // SAFETY: Test runs in isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "EnvAgent");
        }

        // Explicit should win
        let result = resolve_agent(Some("ExplicitAgent"), temp.path());
        assert_eq!(result, Some("ExplicitAgent".to_string()));

        // Clean up
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    fn test_env_var_used_when_no_explicit() {
        let temp = setup_project();

        // SAFETY: Test runs in isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "EnvAgent");
        }

        let result = resolve_agent(None, temp.path());
        assert_eq!(result, Some("EnvAgent".to_string()));

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    fn test_no_identity() {
        let temp = setup_project();

        // Ensure env var is not set
        // SAFETY: Test runs in isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = resolve_agent(None, temp.path());
        assert!(result.is_none());
    }

    #[test]
    fn test_format_export() {
        let export = format_export("MyAgent");
        assert_eq!(export, "export BOTBUS_AGENT=MyAgent");
    }
}
