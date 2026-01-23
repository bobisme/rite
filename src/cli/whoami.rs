use anyhow::{bail, Context, Result};
use colored::Colorize;
use std::path::Path;

use crate::core::agent::Agent;
use crate::core::project::{agents_path, state_path};
use crate::storage::jsonl::read_records;
use crate::storage::state::ProjectState;

/// Display current agent identity.
pub fn run(project_root: &Path) -> Result<()> {
    let state = ProjectState::new(state_path(project_root));
    let current_agent = state
        .current_agent()
        .with_context(|| "Failed to read state")?;

    let agent_name = match current_agent {
        Some(name) => name,
        None => {
            bail!(
                "No agent registered.\n\n\
                 Run 'botbus register' to register an agent identity."
            );
        }
    };

    // Find the agent record for additional info
    let agents: Vec<Agent> =
        read_records(&agents_path(project_root)).with_context(|| "Failed to read agents")?;

    let agent = agents.iter().find(|a| a.name == agent_name);

    println!("{}: {}", "Agent".bold(), agent_name.cyan());
    println!("{}: {}", "Project".bold(), project_root.display());

    if let Some(a) = agent {
        println!(
            "{}: {}",
            "Registered".bold(),
            a.ts.format("%Y-%m-%d %H:%M:%S UTC")
        );
        if let Some(desc) = &a.description {
            println!("{}: {}", "Description".bold(), desc);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{init, register};
    use tempfile::TempDir;

    #[test]
    fn test_whoami_shows_agent() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        register::run(
            Some("TestAgent".to_string()),
            Some("Test Desc".to_string()),
            temp.path(),
        )
        .unwrap();

        // Should not error
        run(temp.path()).unwrap();
    }

    #[test]
    fn test_whoami_no_agent() {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();

        let result = run(temp.path());
        assert!(result.is_err());
    }
}
