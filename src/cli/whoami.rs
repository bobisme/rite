//! Display current agent identity.

use anyhow::{Result, bail};
use colored::Colorize;
use serde::Serialize;
use std::path::{Path, PathBuf};

use super::OutputFormat;
use crate::core::identity::{AGENT_ENV_VAR, resolve_agent};
use crate::core::names::generate_name;
use crate::core::project::data_dir;

/// Find the project root directory by walking up from current directory
/// looking for .git or .jj directories.
fn find_project_root() -> Option<PathBuf> {
    let current_dir = std::env::current_dir().ok()?;
    let mut path = current_dir.as_path();

    loop {
        // Check for .git or .jj directories
        if path.join(".git").exists() || path.join(".jj").exists() {
            return Some(path.to_path_buf());
        }

        // Move up to parent directory
        path = path.parent()?;
    }
}

/// Extract the project name from a path (the last component)
fn get_project_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|s| s.to_string())
}

#[derive(Debug, Serialize)]
pub struct WhoamiOutput {
    pub agent: String,
    pub source: String,
    pub data_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub advice: Vec<String>,
}

/// Display current agent identity.
pub fn run(
    format: OutputFormat,
    agent: Option<&str>,
    suggest_project_suffix: Option<String>,
) -> Result<()> {
    let agent_name = match resolve_agent(agent) {
        Some(name) => name,
        None => {
            // No identity configured - suggest a name
            let suggested_name = if let Some(suffix) = suggest_project_suffix {
                // Try to detect project and suggest <project>-<suffix>
                if let Some(root) = find_project_root() {
                    if let Some(project_name) = get_project_name(&root) {
                        format!("{}-{}", project_name, suffix)
                    } else {
                        generate_name()
                    }
                } else {
                    generate_name()
                }
            } else {
                generate_name()
            };

            let error_msg = match format {
                OutputFormat::Json => {
                    format!(
                        "No agent identity configured. Suggested: {}",
                        suggested_name
                    )
                }
                OutputFormat::Pretty | OutputFormat::Text => {
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
        advice: vec![], // Informational command, no specific next action
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Pretty => {
            println!("{}: {}", "Agent".bold(), agent_name.cyan());
            println!("{}: {}", "Source".bold(), output.source);
            println!("{}: {}", "Data".bold(), data_dir().display());

            if let Some(warn) = warning {
                println!();
                println!("{} {}", "Warning:".yellow().bold(), warn.yellow());
            }
        }
        OutputFormat::Text => {
            println!("{}", agent_name);
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

        run(OutputFormat::Text, None, None).unwrap();

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    fn test_whoami_json() {
        run(OutputFormat::Json, Some("test-agent"), None).unwrap();
    }

    #[test]
    #[serial]
    fn test_whoami_no_agent() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = run(OutputFormat::Text, None, None);
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_whoami_suggest_project_suffix() {
        // SAFETY: Test isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        // If we're in a git/jj project, the error message should contain the project name
        let result = run(OutputFormat::Text, None, Some("dev".to_string()));
        assert!(result.is_err());

        // Check that the error message contains a suggested name and "botbus-dev"
        let err_msg = result.unwrap_err().to_string();
        eprintln!("Error message: {}", err_msg);
        assert!(err_msg.contains("botbus-dev") || err_msg.contains("suggested"));
    }

    #[test]
    fn test_find_project_root() {
        // Should find the botbus project root
        let root = find_project_root();
        assert!(root.is_some());

        if let Some(root) = root {
            assert!(root.join(".git").exists() || root.join(".jj").exists());
        }
    }

    #[test]
    fn test_get_project_name() {
        use std::path::PathBuf;

        let path = PathBuf::from("/home/user/projects/my-project");
        let name = get_project_name(&path);
        assert_eq!(name, Some("my-project".to_string()));

        let path = PathBuf::from("/");
        let name = get_project_name(&path);
        // Root should have no file name component in most cases
        assert!(name.is_none() || name == Some("".to_string()));
    }
}
