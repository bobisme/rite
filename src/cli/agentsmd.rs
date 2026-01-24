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

/// Options for the agentsmd command
#[derive(Debug, Clone)]
pub struct AgentsMdOptions {
    pub add: bool,
    pub remove: bool,
    pub update: bool,
    pub check: bool,
    pub show: bool,
    pub dry_run: bool,
    pub force: bool,
    pub file: Option<PathBuf>,
}

impl Default for AgentsMdOptions {
    fn default() -> Self {
        Self {
            add: false,
            remove: false,
            update: false,
            check: true, // default action
            show: false,
            dry_run: false,
            force: false,
            file: None,
        }
    }
}

/// Result of checking for existing instructions
#[derive(Debug)]
pub enum InstructionsStatus {
    /// Found instructions in a file
    Found { path: PathBuf, current: bool },
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
pub fn check_instructions(path: &Path) -> Result<Option<(usize, usize)>> {
    let content = std::fs::read_to_string(path).context("Failed to read file")?;

    let start_pos = content.find(MARKER_START);
    let end_pos = content.find(MARKER_END);

    match (start_pos, end_pos) {
        (Some(start), Some(end)) if end > start => {
            // Find line numbers
            let start_line = content[..start].matches('\n').count();
            let end_line = content[..end].matches('\n').count();
            Ok(Some((start_line + 1, end_line + 1)))
        }
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("Malformed BotBus instructions: mismatched markers")
        }
        _ => Ok(None),
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

    match check_instructions(&path)? {
        Some(_) => Ok(InstructionsStatus::Found {
            path,
            current: true, // TODO: version checking
        }),
        None => Ok(InstructionsStatus::NotFound { path }),
    }
}

/// Add instructions to a file
pub fn add_instructions(path: &Path, dry_run: bool) -> Result<String> {
    let content = if path.exists() {
        std::fs::read_to_string(path).context("Failed to read file")?
    } else {
        String::new()
    };

    // Check if already present
    if content.contains(MARKER_START) {
        anyhow::bail!(
            "BotBus instructions already exist in {}. Use --update to modify.",
            path.display()
        );
    }

    let instructions = get_instructions_content();

    if dry_run {
        return Ok(format!(
            "Would add BotBus instructions to: {}\n\n--- Preview ---\n{}",
            path.display(),
            instructions
        ));
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
    Ok(format!("Added BotBus instructions to: {}", path.display()))
}

/// Remove instructions from a file
pub fn remove_instructions(path: &Path, dry_run: bool) -> Result<String> {
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

            if dry_run {
                Ok(format!(
                    "Would remove BotBus instructions from: {}",
                    path.display()
                ))
            } else {
                std::fs::write(path, &new_content).context("Failed to write file")?;
                Ok(format!(
                    "Removed BotBus instructions from: {}",
                    path.display()
                ))
            }
        }
        _ => anyhow::bail!("No BotBus instructions found in {}", path.display()),
    }
}

/// Update instructions in a file
pub fn update_instructions(path: &Path, dry_run: bool) -> Result<String> {
    let content = std::fs::read_to_string(path).context("Failed to read file")?;

    let start_pos = content.find(MARKER_START);
    let end_pos = content.find(MARKER_END);

    match (start_pos, end_pos) {
        (Some(start), Some(end)) if end > start => {
            let end = end + MARKER_END.len();

            let before = &content[..start];
            let after = &content[end..];
            let instructions = get_instructions_content();

            let new_content = format!("{before}{instructions}{after}");

            if dry_run {
                Ok(format!(
                    "Would update BotBus instructions in: {}\n\n--- Preview ---\n{}",
                    path.display(),
                    instructions
                ))
            } else {
                std::fs::write(path, &new_content).context("Failed to write file")?;
                Ok(format!(
                    "Updated BotBus instructions in: {}",
                    path.display()
                ))
            }
        }
        _ => anyhow::bail!(
            "No BotBus instructions found in {}. Use --add to create.",
            path.display()
        ),
    }
}

/// Run the agentsmd command
pub fn run(options: AgentsMdOptions, project_root: &Path) -> Result<()> {
    // Handle --show first (doesn't need a file)
    if options.show {
        println!("{}", get_instructions_content());
        return Ok(());
    }

    // Determine which action to take (mutually exclusive, with defaults)
    let action = if options.add {
        "add"
    } else if options.remove {
        "remove"
    } else if options.update {
        "update"
    } else {
        "check"
    };

    // Find or use explicit file
    let explicit_file = options.file.as_deref();
    let status = get_status(project_root, explicit_file)?;

    match action {
        "check" => match &status {
            InstructionsStatus::Found { path, current } => {
                println!(
                    "Found: {} at {}",
                    if *current {
                        "BotBus instructions (current)"
                    } else {
                        "BotBus instructions (outdated)"
                    },
                    path.display()
                );
                if !current {
                    println!("\nTo update:\n  botbus agentsmd --update");
                }
            }
            InstructionsStatus::NotFound { path } => {
                println!(
                    "Found: {} at {}",
                    path.file_name().unwrap().to_string_lossy(),
                    path.display()
                );
                println!("\nStatus: No BotBus instructions found");
                println!("\nTo add:\n  botbus agentsmd --add");
            }
            InstructionsStatus::NoFile => {
                println!("No agent instructions file found (AGENTS.md, CLAUDE.md, etc.)");
                println!("\nTo create AGENTS.md with BotBus instructions:");
                println!("  botbus agentsmd --add --file AGENTS.md");
            }
        },
        "add" => {
            let path = match &status {
                InstructionsStatus::Found { path, .. } => {
                    anyhow::bail!(
                        "BotBus instructions already exist in {}. Use --update to modify.",
                        path.display()
                    );
                }
                InstructionsStatus::NotFound { path } => path.clone(),
                InstructionsStatus::NoFile => {
                    if let Some(f) = explicit_file {
                        f.to_path_buf()
                    } else {
                        // Default to AGENTS.md
                        project_root.join("AGENTS.md")
                    }
                }
            };

            if !options.force && !options.dry_run {
                eprintln!("Will add BotBus instructions to: {}", path.display());
                eprintln!("Use --dry-run to preview or --force to skip confirmation.");
                // In a real implementation, we'd prompt here
                // For now, just proceed
            }

            let result = add_instructions(&path, options.dry_run)?;
            println!("{result}");
        }
        "remove" => {
            let path = match &status {
                InstructionsStatus::Found { path, .. } => path.clone(),
                InstructionsStatus::NotFound { path } => {
                    anyhow::bail!("No BotBus instructions found in {}", path.display());
                }
                InstructionsStatus::NoFile => {
                    anyhow::bail!("No agent instructions file found");
                }
            };

            if !options.force && !options.dry_run {
                eprintln!("Will remove BotBus instructions from: {}", path.display());
                eprintln!("Use --dry-run to preview or --force to skip confirmation.");
            }

            let result = remove_instructions(&path, options.dry_run)?;
            println!("{result}");
        }
        "update" => {
            let path = match &status {
                InstructionsStatus::Found { path, .. } => path.clone(),
                InstructionsStatus::NotFound { path } => {
                    anyhow::bail!(
                        "No BotBus instructions found in {}. Use --add to create.",
                        path.display()
                    );
                }
                InstructionsStatus::NoFile => {
                    anyhow::bail!("No agent instructions file found. Use --add to create.");
                }
            };

            if !options.force && !options.dry_run {
                eprintln!("Will update BotBus instructions in: {}", path.display());
                eprintln!("Use --dry-run to preview or --force to skip confirmation.");
            }

            let result = update_instructions(&path, options.dry_run)?;
            println!("{result}");
        }
        _ => unreachable!(),
    }

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
        assert!(result.is_some());
    }

    #[test]
    fn test_check_instructions_absent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "# Agents\n\nNo botbus here").unwrap();

        let result = check_instructions(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_add_instructions() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "# Agents\n\nExisting content").unwrap();

        let result = add_instructions(&path, false).unwrap();
        assert!(result.contains("Added"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER_START));
        assert!(content.contains(MARKER_END));
        assert!(content.contains("Existing content"));
    }

    #[test]
    fn test_add_instructions_dry_run() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "# Agents").unwrap();

        let result = add_instructions(&path, true).unwrap();
        assert!(result.contains("Would add"));

        // File should be unchanged
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains(MARKER_START));
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

        let result = remove_instructions(&path, false).unwrap();
        assert!(result.contains("Removed"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains(MARKER_START));
        assert!(!content.contains(MARKER_END));
        assert!(content.contains("# Agents"));
        assert!(content.contains("More content"));
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

        let result = update_instructions(&path, false).unwrap();
        assert!(result.contains("Updated"));

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(MARKER_START));
        assert!(content.contains(MARKER_END));
        assert!(content.contains("BotBus Agent Coordination")); // New content
        assert!(!content.contains("Old content"));
        assert!(content.contains("Other stuff")); // Preserved
    }
}
