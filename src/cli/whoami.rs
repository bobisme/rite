use anyhow::{bail, Context, Result};
use colored::Colorize;
use serde::Serialize;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::identity::{format_export, resolve_agent, AGENT_ENV_VAR};
use crate::core::project::agents_path;
use crate::storage::jsonl::read_records;

#[derive(Debug, Serialize)]
pub struct WhoamiOutput {
    pub agent: String,
    pub project: String,
    pub source: String,
    pub registered: bool,
    pub description: Option<String>,
}

/// Display current agent identity.
pub fn run(json: bool, agent: Option<&str>, project_root: &Path) -> Result<()> {
    let agent_name = match resolve_agent(agent, project_root) {
        Some(name) => name,
        None => {
            if json {
                println!("{{\"error\": \"No agent identity configured\"}}");
                return Ok(());
            }
            bail!(
                "No agent identity configured.\n\n\
                 To set your identity:\n\
                 1. Run 'botbus register' to create a new identity\n\
                 2. Set the environment variable: {}\n\n\
                 Or use --agent flag with commands.",
                format_export("YourAgentName")
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
        "state.json".to_string()
    };

    // Find the agent record for additional info
    let agents: Vec<Agent> =
        read_records(&agents_path(project_root)).with_context(|| "Failed to read agents")?;

    let agent_record = agents.iter().find(|a| a.name == agent_name);

    if json {
        let output = WhoamiOutput {
            agent: agent_name,
            project: project_root.display().to_string(),
            source,
            registered: agent_record.is_some(),
            description: agent_record.and_then(|a| a.description.clone()),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("{}: {}", "Agent".bold(), agent_name.cyan());
    println!("{}: {}", "Project".bold(), project_root.display());
    println!("{}: {}", "Source".bold(), source);

    if let Some(a) = agent_record {
        println!(
            "{}: {}",
            "Registered".bold(),
            a.ts.format("%Y-%m-%d %H:%M:%S UTC")
        );
        if let Some(desc) = &a.description {
            println!("{}: {}", "Description".bold(), desc);
        }
    } else {
        println!(
            "{}: {} (not found in agents.jsonl)",
            "Warning".yellow(),
            "Agent not registered in this project"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init;
    use std::env;
    use tempfile::TempDir;

    #[test]
    fn test_whoami_shows_agent() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();

        // Set env var to simulate registered agent
        // SAFETY: Test runs in isolation
        unsafe {
            env::set_var(AGENT_ENV_VAR, "TestAgent");
        }

        run(false, None, temp.path()).unwrap();

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
    }

    #[test]
    fn test_whoami_json() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();

        run(true, Some("TestAgent"), temp.path()).unwrap();
    }

    #[test]
    fn test_whoami_no_agent() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();

        // Ensure no env var
        // SAFETY: Test runs in isolation
        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }

        let result = run(false, None, temp.path());
        assert!(result.is_err());
    }
}
