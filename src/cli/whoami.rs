//! Display current agent identity.

use anyhow::{Result, bail};
use colored::Colorize;
use serde::Serialize;

use crate::core::identity::{AGENT_ENV_VAR, format_export, resolve_agent};
use crate::core::project::data_dir;

#[derive(Debug, Serialize)]
pub struct WhoamiOutput {
    pub agent: String,
    pub source: String,
    pub data_dir: String,
}

/// Display current agent identity.
pub fn run(json: bool, agent: Option<&str>) -> Result<()> {
    let agent_name = match resolve_agent(agent) {
        Some(name) => name,
        None => {
            if json {
                println!("{{\"error\": \"No agent identity configured\"}}");
                return Ok(());
            }
            bail!(
                "No agent identity configured.\n\n\
                 To set your identity:\n  \
                 export BOTBUS_AGENT=$(botbus generate-name)\n\n\
                 Or choose your own name (kebab-case preferred):\n  \
                 {}\n\n\
                 Or use --agent flag with commands.",
                format_export("my-agent-name")
            );
        }
    };

    // Check where identity came from
    let from_env = std::env::var(AGENT_ENV_VAR).ok().as_deref() == Some(&agent_name);
    let source = if agent.is_some() {
        "--agent flag".to_string()
    } else if from_env {
        format!("${}", AGENT_ENV_VAR)
    } else {
        "unknown".to_string()
    };

    if json {
        let output = WhoamiOutput {
            agent: agent_name,
            source,
            data_dir: data_dir().display().to_string(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("{}: {}", "Agent".bold(), agent_name.cyan());
    println!("{}: {}", "Source".bold(), source);
    println!("{}: {}", "Data".bold(), data_dir().display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    #[test]
    #[serial]
    fn test_whoami_shows_agent() {
        // SAFETY: Test isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "test-agent");
        }

        run(false, None).unwrap();

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    fn test_whoami_json() {
        run(true, Some("test-agent")).unwrap();
    }

    #[test]
    #[serial]
    fn test_whoami_no_agent() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = run(false, None);
        assert!(result.is_err());
    }
}
