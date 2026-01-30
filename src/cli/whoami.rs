//! Display current agent identity.

use anyhow::{Result, bail};
use colored::Colorize;
use serde::Serialize;

use super::OutputFormat;
use super::format::to_toon;
use crate::core::identity::{AGENT_ENV_VAR, resolve_agent};
use crate::core::names::generate_name;
use crate::core::project::data_dir;

#[derive(Debug, Serialize)]
pub struct WhoamiOutput {
    pub agent: String,
    pub source: String,
    pub data_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Display current agent identity.
pub fn run(format: OutputFormat, agent: Option<&str>) -> Result<()> {
    let agent_name = match resolve_agent(agent) {
        Some(name) => name,
        None => {
            // No identity configured - suggest a random name
            let suggested_name = generate_name();

            let error_msg = match format {
                OutputFormat::Json | OutputFormat::Toon => {
                    format!(
                        "No agent identity configured. Suggested: {}",
                        suggested_name
                    )
                }
                OutputFormat::Text => {
                    format!(
                        "{}\n\n\
                         {} Here is a random identity you could use:\n\n  \
                         {}\n\n\
                         To use it with --agent flag (recommended for agents/scripts):\n  \
                         botbus --agent {} <command>\n\n\
                         Or set in environment (for interactive shells):\n  \
                         export BOTBUS_AGENT={}\n\n\
                         Or generate a different name:\n  \
                         botbus generate-name\n\n\
                         Note: Environment variables don't persist in sandboxed environments.\n  \
                         Use --agent flag for reliable identity across commands.",
                        "Error: No agent identity detected.".red().bold(),
                        "→".cyan().bold(),
                        suggested_name.green().bold(),
                        suggested_name,
                        suggested_name
                    )
                }
            };
            bail!("{}", error_msg);
        }
    };

    // Check where identity came from
    let env_value = std::env::var(AGENT_ENV_VAR).ok();
    let from_env = env_value.as_deref() == Some(&agent_name);

    // Determine if --agent was explicitly used (different from env or env not set)
    let from_explicit_flag = match (agent, env_value.as_deref()) {
        (Some(arg), Some(env)) => arg != env, // --agent differs from env
        (Some(_), None) => true,              // --agent used, no env set
        (None, _) => false,                   // No --agent flag
    };

    let source = if from_env && !from_explicit_flag {
        format!("${}", AGENT_ENV_VAR)
    } else if from_explicit_flag {
        "--agent flag".to_string()
    } else {
        "unknown".to_string()
    };

    let warning = if from_explicit_flag {
        Some("whoami is intended to check environment identity. Using --agent flag defeats this purpose.".to_string())
    } else {
        None
    };

    let output = WhoamiOutput {
        agent: agent_name.clone(),
        source,
        data_dir: data_dir().display().to_string(),
        warning: warning.clone(),
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

            if let Some(warn) = warning {
                println!();
                println!("{} {}", "Warning:".yellow().bold(), warn.yellow());
            }
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
