//! AGENTS.md management subcommand
//!
//! Generates and manages BotBus workflow instructions in AGENTS.md or similar files.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Marker comments for identifying the BotBus section
const MARKER_START: &str = "<!-- botbus-agent-instructions-v1 -->";
const MARKER_END: &str = "<!-- end-botbus-agent-instructions -->";

/// Files to search for (in priority order)
const AGENT_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".claude/CLAUDE.md",
    ".claude/settings.md",
    "agents.md",
    "claude.md",
    ".cursor/rules.md",
];

/// The BotBus instructions content to insert
fn get_instructions_content() -> String {
    format!(
        r#"{MARKER_START}

## BotBus Agent Coordination

This project uses [BotBus](https://github.com/anomalyco/botbus) for multi-agent coordination. Before starting work, check for other agents and active file claims.

### Quick Start

```bash
# Register yourself (once per session)
botbus register --name YourAgentName --description "Brief description"

# Check what's happening
botbus status              # Overview of project state
botbus history             # Recent messages
botbus agents              # Who's registered

# Communicate
botbus send general "Starting work on X"
botbus send general "Done with X, ready for review"
botbus send @OtherAgent "Question about Y"

# Coordinate file access
botbus claim "src/api/**" -m "Working on API routes"
botbus check-claim src/api/routes.rs   # Before editing
botbus release --all                    # When done
```

### Best Practices

1. **Announce your intent** before starting significant work
2. **Claim files** you plan to edit to avoid conflicts
3. **Check claims** before editing files outside your claimed area
4. **Send updates** on blockers, questions, or completed work
5. **Release claims** when done - don't hoard files

### Channel Conventions

- `#general` - Default channel for project-wide updates
- `#backend`, `#frontend`, etc. - Create topic channels as needed
- `@AgentName` - Direct messages for specific coordination

### Message Conventions

Keep messages concise and actionable:
- "Starting work on issue #123: Add foo feature"
- "Blocked: need database credentials to proceed"
- "Question: should auth middleware go in src/api or src/auth?"
- "Done: implemented bar, tests passing"

{MARKER_END}"#
    )
}

/// Result of checking for existing instructions
#[derive(Debug)]
pub enum InstructionsStatus {
    /// Found instructions in a file
    Found { path: PathBuf },
    /// No instructions found, but found a candidate file
    NotFound { path: PathBuf },
    /// No agent file found at all
    NoFile,
}

/// Find the agent instructions file in the project
pub fn find_agent_file(project_root: &Path) -> Option<PathBuf> {
    for filename in AGENT_FILES {
        let path = project_root.join(filename);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Check if the file contains BotBus instructions
pub fn check_instructions(path: &Path) -> Result<bool> {
    let content = std::fs::read_to_string(path).context("Failed to read file")?;
    let has_start = content.contains(MARKER_START);
    let has_end = content.contains(MARKER_END);

    match (has_start, has_end) {
        (true, true) => Ok(true),
        (false, false) => Ok(false),
        _ => anyhow::bail!("Malformed BotBus instructions: mismatched markers"),
    }
}

/// Get the status of instructions in the project
pub fn get_status(project_root: &Path, explicit_file: Option<&Path>) -> Result<InstructionsStatus> {
    let path = if let Some(f) = explicit_file {
        if f.exists() {
            f.to_path_buf()
        } else {
            return Ok(InstructionsStatus::NoFile);
        }
    } else {
        match find_agent_file(project_root) {
            Some(p) => p,
            None => return Ok(InstructionsStatus::NoFile),
        }
    };

    if check_instructions(&path)? {
        Ok(InstructionsStatus::Found { path })
    } else {
        Ok(InstructionsStatus::NotFound { path })
    }
}

/// Add or update instructions in a file
fn add_or_update_instructions(path: &Path) -> Result<String> {
    let content = if path.exists() {
        std::fs::read_to_string(path).context("Failed to read file")?
    } else {
        String::new()
    };

    let instructions = get_instructions_content();

    // Check if already present - if so, update
    if content.contains(MARKER_START) && content.contains(MARKER_END) {
        let start_pos = content.find(MARKER_START).unwrap();
        let end_pos = content.find(MARKER_END).unwrap() + MARKER_END.len();

        let before = &content[..start_pos];
        let after = &content[end_pos..];
        let new_content = format!("{before}{instructions}{after}");

        std::fs::write(path, &new_content).context("Failed to write file")?;
        return Ok(format!("Updated BotBus section in {}", path.display()));
    }

    // Add to end of file with a separator
    let new_content = if content.is_empty() {
        instructions
    } else if content.ends_with('\n') {
        format!("{content}\n---\n\n{instructions}\n")
    } else {
        format!("{content}\n\n---\n\n{instructions}\n")
    };

    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create parent directories")?;
    }
    std::fs::write(path, &new_content).context("Failed to write file")?;
    Ok(format!("Added BotBus section to {}", path.display()))
}

/// Remove instructions from a file
fn remove_instructions(path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(path).context("Failed to read file")?;

    let start_pos = content.find(MARKER_START);
    let end_pos = content.find(MARKER_END);

    match (start_pos, end_pos) {
        (Some(start), Some(end)) if end > start => {
            let end = end + MARKER_END.len();

            // Remove the section and clean up extra whitespace/separators
            let before = content[..start].trim_end();
            let after = content[end..].trim_start();

            // Clean up separator if present
            let after = after.strip_prefix("---").unwrap_or(after).trim_start();
            let before = before.strip_suffix("---").unwrap_or(before).trim_end();

            let new_content = if before.is_empty() {
                after.to_string()
            } else if after.is_empty() {
                format!("{before}\n")
            } else {
                format!("{before}\n\n{after}")
            };

            std::fs::write(path, &new_content).context("Failed to write file")?;
            Ok(format!("Removed BotBus section from {}", path.display()))
        }
        _ => anyhow::bail!("No BotBus instructions found in {}", path.display()),
    }
}

/// Run the 'init' subcommand - add or update instructions
pub fn run_init(file: Option<PathBuf>, remove: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;
    let status = get_status(&cwd, file.as_deref())?;

    if remove {
        // Handle remove
        let path = match &status {
            InstructionsStatus::Found { path } => path.clone(),
            InstructionsStatus::NotFound { path } => {
                anyhow::bail!("No BotBus instructions found in {}", path.display());
            }
            InstructionsStatus::NoFile => {
                anyhow::bail!("No agent instructions file found");
            }
        };
        let result = remove_instructions(&path)?;
        println!("{result}");
    } else {
        // Handle add/update
        let path = match &status {
            InstructionsStatus::Found { path } => path.clone(),
            InstructionsStatus::NotFound { path } => path.clone(),
            InstructionsStatus::NoFile => {
                if let Some(f) = file {
                    f
                } else {
                    // Default to AGENTS.md in current directory
                    cwd.join("AGENTS.md")
                }
            }
        };
        let result = add_or_update_instructions(&path)?;
        println!("{result}");
    }

    Ok(())
}

/// Run the 'show' subcommand - print the instructions content
pub fn run_show() -> Result<()> {
    println!("{}", get_instructions_content());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_agent_file_agents_md() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), "# Agents").unwrap();

        let found = find_agent_file(tmp.path());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("AGENTS.md"));
    }

    #[test]
    fn test_find_agent_file_claude_md() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "# Claude").unwrap();

        let found = find_agent_file(tmp.path());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("CLAUDE.md"));
    }

    #[test]
    fn test_find_agent_file_priority() {
        let tmp = TempDir::new().unwrap();
        // Both exist, AGENTS.md should win
        std::fs::write(tmp.path().join("AGENTS.md"), "# Agents").unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "# Claude").unwrap();

        let found = find_agent_file(tmp.path());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("AGENTS.md"));
    }

    #[test]
    fn test_find_agent_file_none() {
        let tmp = TempDir::new().unwrap();
        let found = find_agent_file(tmp.path());
        assert!(found.is_none());
    }

    #[test]
    fn test_check_instructions_present() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let content = format!(
            "# Agents\n\n{}\n\nSome content\n\n{}\n\nMore stuff",
            MARKER_START, MARKER_END
        );
        std::fs::write(&path, content).unwrap();

        let result = check_instructions(&path).unwrap();
        assert!(result);
    }

    #[test]
    fn test_check_instructions_absent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "# Agents\n\nNo botbus here").unwrap();

        let result = check_instructions(&path).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_add_instructions() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "# Agents\n\nExisting content").unwrap();

        let result = add_or_update_instructions(&path).unwrap();
        assert!(result.contains("Added"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER_START));
        assert!(content.contains(MARKER_END));
        assert!(content.contains("Existing content"));
    }

    #[test]
    fn test_update_instructions() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let content = format!(
            "# Agents\n\n{}\nOld content\n{}\n\nOther stuff",
            MARKER_START, MARKER_END
        );
        std::fs::write(&path, content).unwrap();

        let result = add_or_update_instructions(&path).unwrap();
        assert!(result.contains("Updated"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER_START));
        assert!(content.contains(MARKER_END));
        assert!(content.contains("BotBus Agent Coordination")); // New content
        assert!(!content.contains("Old content"));
        assert!(content.contains("Other stuff")); // Preserved
    }

    #[test]
    fn test_remove_instructions() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let content = format!(
            "# Agents\n\n---\n\n{}\nBotBus stuff\n{}\n\n---\n\nMore content",
            MARKER_START, MARKER_END
        );
        std::fs::write(&path, content).unwrap();

        let result = remove_instructions(&path).unwrap();
        assert!(result.contains("Removed"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains(MARKER_START));
        assert!(!content.contains(MARKER_END));
        assert!(content.contains("# Agents"));
        assert!(content.contains("More content"));
    }

    #[test]
    fn test_init_creates_new_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");

        // Test via add_or_update_instructions directly since run_init uses cwd
        let result = add_or_update_instructions(&path).unwrap();
        assert!(result.contains("Added"));

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER_START));
    }

    #[test]
    fn test_init_updates_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let content = format!("# Agents\n\n{}\nOld\n{}", MARKER_START, MARKER_END);
        std::fs::write(&path, content).unwrap();

        // Test via add_or_update_instructions directly since run_init uses cwd
        add_or_update_instructions(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("BotBus Agent Coordination"));
        assert!(!content.contains("Old"));
    }
}
