use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::identity::{format_export, resolve_agent, AGENT_ENV_VAR};
use crate::core::project::agents_path;
use crate::storage::jsonl::read_records;

/// Display current agent identity.
pub fn run(agent: Option<&str>, project_root: &Path) -> Result<()> {
    let agent_name = match resolve_agent(agent, project_root) {
        Some(name) => name,
        None => {
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

    // Find the agent record for additional info
    let agents: Vec<Agent> =
        read_records(&agents_path(project_root)).with_context(|| "Failed to read agents")?;

    let agent = agents.iter().find(|a| a.name == agent_name);

    println!("{}: {}", "Agent".bold(), agent_name.cyan());
    println!("{}: {}", "Project".bold(), project_root.display());

    if from_env {
        println!("{}: {}", "Source".bold(), format!("${}", AGENT_ENV_VAR));
    }

    if let Some(a) = agent {
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

        // Should not error (though agent won't be in agents.jsonl)
        run(None, temp.path()).unwrap();

        unsafe {
            env::remove_var(AGENT_ENV_VAR);
        }
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

        let result = run(None, temp.path());
        assert!(result.is_err());
    }
}
