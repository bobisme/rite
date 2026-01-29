//! Display current agent identity.

use anyhow::{Result, bail};
use colored::Colorize;
use serde::Serialize;

use super::format::to_toon;
use super::OutputFormat;
use crate::core::identity::{AGENT_ENV_VAR, format_export, resolve_agent};
use crate::core::project::data_dir;

#[derive(Debug, Serialize)]
pub struct WhoamiOutput {
    pub agent: String,
    pub source: String,
    pub data_dir: String,
}

/// Display current agent identity.
pub fn run(format: OutputFormat, agent: Option<&str>) -> Result<()> {
    let agent_name = match resolve_agent(agent) {
        Some(name) => name,
        None => {
            match format {
                OutputFormat::Json => {
                    println!("{{\"error\": \"No agent identity configured\"}}");
                    return Ok(());
                }
                OutputFormat::Toon => {
                    println!("error: No agent identity configured");
                    return Ok(());
                }
                OutputFormat::Text => {
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
            }
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

    let output = WhoamiOutput {
        agent: agent_name.clone(),
        source,
        data_dir: data_dir().display().to_string(),
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Toon => {
            println!("{}", to_toon(&output));
        }
        OutputFormat::Text => {
            println!("{}: {}", "Agent".bold(), agent_name.cyan());
            println!("{}: {}", "Source".bold(), output.source);
            println!("{}: {}", "Data".bold(), data_dir().display());
        }
    }

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

        run(OutputFormat::Text, None).unwrap();

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    fn test_whoami_json() {
        run(OutputFormat::Json, Some("test-agent")).unwrap();
    }

    #[test]
    #[serial]
    fn test_whoami_no_agent() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = run(OutputFormat::Text, None);
        assert!(result.is_err());
    }
}
