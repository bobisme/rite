//! Doctor command for environment validation.
//!
//! Checks that the BotBus environment is properly configured for agent use.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::OutputFormat;
use super::format::to_toon;
use crate::core::identity::resolve_agent;
use crate::core::names::is_valid_name;
use crate::core::project::{channels_dir, claims_path, data_dir, index_path, state_path};

/// A single check result.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Full doctor report.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
    pub pass_count: usize,
    pub warn_count: usize,
    pub fail_count: usize,
}

impl DoctorReport {
    fn new() -> Self {
        Self {
            checks: Vec::new(),
            pass_count: 0,
            warn_count: 0,
            fail_count: 0,
        }
    }

    fn add(&mut self, check: Check) {
        match check.status {
            CheckStatus::Pass => self.pass_count += 1,
            CheckStatus::Warn => self.warn_count += 1,
            CheckStatus::Fail => self.fail_count += 1,
        }
        self.checks.push(check);
    }

    fn is_healthy(&self) -> bool {
        self.fail_count == 0
    }
}

/// Run all doctor checks.
pub fn run(format: OutputFormat) -> Result<()> {
    let mut report = DoctorReport::new();

    // Check 1: Data directory exists
    check_data_dir(&mut report);

    // Check 2: Agent identity is set
    check_agent_identity(&mut report);

    // Check 3: Channels directory is writable
    check_channels_dir(&mut report);

    // Check 4: Claims file location is writable
    check_claims(&mut report);

    // Check 5: State file location is writable
    check_state(&mut report);

    // Check 6: Index (FTS) location
    check_index(&mut report);

    // Check 7: Data directory permissions (security)
    check_permissions(&mut report);

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        OutputFormat::Toon => {
            println!("{}", to_toon(&report));
        }
        OutputFormat::Text => {
            print_report(&report);
        }
    }

    if !report.is_healthy() {
        std::process::exit(1);
    }

    Ok(())
}

fn check_data_dir(report: &mut DoctorReport) {
    let path = data_dir();
    if path.exists() {
        report.add(Check {
            name: "data_directory".to_string(),
            status: CheckStatus::Pass,
            message: format!("Data directory exists: {}", path.display()),
            suggestion: None,
        });
    } else {
        report.add(Check {
            name: "data_directory".to_string(),
            status: CheckStatus::Fail,
            message: format!("Data directory missing: {}", path.display()),
            suggestion: Some("Run: botbus init".to_string()),
        });
    }
}

fn check_agent_identity(report: &mut DoctorReport) {
    match resolve_agent(None) {
        Some(ref agent) => {
            if is_valid_name(agent) {
                report.add(Check {
                    name: "agent_identity".to_string(),
                    status: CheckStatus::Pass,
                    message: format!("Agent identity: {}", agent),
                    suggestion: None,
                });
            } else {
                report.add(Check {
                    name: "agent_identity".to_string(),
                    status: CheckStatus::Warn,
                    message: format!("Agent name '{}' is not valid kebab-case", agent),
                    suggestion: Some(
                        "Use: export BOTBUS_AGENT=$(botbus generate-name)".to_string(),
                    ),
                });
            }
        }
        None => {
            report.add(Check {
                name: "agent_identity".to_string(),
                status: CheckStatus::Warn,
                message: "No agent identity set (BOTBUS_AGENT not defined)".to_string(),
                suggestion: Some("Run: export BOTBUS_AGENT=$(botbus generate-name)".to_string()),
            });
        }
    }
}

fn check_channels_dir(report: &mut DoctorReport) {
    let path = channels_dir();
    if path.exists() {
        if is_writable(&path) {
            report.add(Check {
                name: "channels_directory".to_string(),
                status: CheckStatus::Pass,
                message: "Channels directory is writable".to_string(),
                suggestion: None,
            });
        } else {
            report.add(Check {
                name: "channels_directory".to_string(),
                status: CheckStatus::Fail,
                message: format!("Channels directory not writable: {}", path.display()),
                suggestion: Some(format!("Check permissions on {}", path.display())),
            });
        }
    } else {
        report.add(Check {
            name: "channels_directory".to_string(),
            status: CheckStatus::Fail,
            message: "Channels directory missing".to_string(),
            suggestion: Some("Run: botbus init".to_string()),
        });
    }
}

fn check_claims(report: &mut DoctorReport) {
    let path = claims_path();
    if let Some(parent) = path.parent() {
        if parent.exists() {
            if is_writable(parent) {
                report.add(Check {
                    name: "claims_storage".to_string(),
                    status: CheckStatus::Pass,
                    message: "Claims storage location is writable".to_string(),
                    suggestion: None,
                });
            } else {
                report.add(Check {
                    name: "claims_storage".to_string(),
                    status: CheckStatus::Fail,
                    message: "Claims storage location not writable".to_string(),
                    suggestion: Some(format!("Check permissions on {}", parent.display())),
                });
            }
        } else {
            report.add(Check {
                name: "claims_storage".to_string(),
                status: CheckStatus::Fail,
                message: "Claims directory missing".to_string(),
                suggestion: Some("Run: botbus init".to_string()),
            });
        }
    }
}

fn check_state(report: &mut DoctorReport) {
    let path = state_path();
    if let Some(parent) = path.parent()
        && parent.exists()
    {
        if is_writable(parent) {
            report.add(Check {
                name: "state_storage".to_string(),
                status: CheckStatus::Pass,
                message: "State storage location is writable".to_string(),
                suggestion: None,
            });
        } else {
            report.add(Check {
                name: "state_storage".to_string(),
                status: CheckStatus::Fail,
                message: "State storage location not writable".to_string(),
                suggestion: Some(format!("Check permissions on {}", parent.display())),
            });
        }
    }
}

fn check_index(report: &mut DoctorReport) {
    let path = index_path();
    if let Some(parent) = path.parent()
        && parent.exists()
    {
        if is_writable(parent) {
            let index_exists = path.exists();
            report.add(Check {
                name: "search_index".to_string(),
                status: CheckStatus::Pass,
                message: if index_exists {
                    "Search index exists and location is writable".to_string()
                } else {
                    "Search index location is writable (index not yet created)".to_string()
                },
                suggestion: None,
            });
        } else {
            report.add(Check {
                name: "search_index".to_string(),
                status: CheckStatus::Warn,
                message: "Search index location not writable".to_string(),
                suggestion: Some(format!("Check permissions on {}", parent.display())),
            });
        }
    }
}

fn check_permissions(report: &mut DoctorReport) {
    let path = data_dir();
    if !path.exists() {
        return; // Already reported in check_data_dir
    }

    match fs::metadata(&path) {
        Ok(meta) => {
            let mode = meta.permissions().mode();
            // Check if group/other have write access (security concern)
            let group_write = mode & 0o020 != 0;
            let other_write = mode & 0o002 != 0;

            if group_write || other_write {
                report.add(Check {
                    name: "permissions".to_string(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "Data directory has permissive permissions: {:o}",
                        mode & 0o777
                    ),
                    suggestion: Some(format!("Consider: chmod 700 {}", path.display())),
                });
            } else {
                report.add(Check {
                    name: "permissions".to_string(),
                    status: CheckStatus::Pass,
                    message: format!("Data directory permissions: {:o}", mode & 0o777),
                    suggestion: None,
                });
            }
        }
        Err(e) => {
            report.add(Check {
                name: "permissions".to_string(),
                status: CheckStatus::Warn,
                message: format!("Could not check permissions: {}", e),
                suggestion: None,
            });
        }
    }
}

fn is_writable(path: &Path) -> bool {
    // Try to check write permission
    match fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.permissions().mode();
            // Check if current user has write permission
            // This is a simplified check - proper check would need to verify uid/gid
            (mode & 0o200) != 0 || (mode & 0o020) != 0 || (mode & 0o002) != 0
        }
        Err(_) => false,
    }
}

fn print_report(report: &DoctorReport) {
    println!("{}", "BotBus Doctor".bold());
    println!();

    for check in &report.checks {
        let (icon, color) = match check.status {
            CheckStatus::Pass => ("✓", "green"),
            CheckStatus::Warn => ("!", "yellow"),
            CheckStatus::Fail => ("✗", "red"),
        };

        let icon_colored = match color {
            "green" => icon.green(),
            "yellow" => icon.yellow(),
            "red" => icon.red(),
            _ => icon.normal(),
        };

        println!("{} {}", icon_colored, check.message);

        if let Some(ref suggestion) = check.suggestion {
            println!("  {} {}", "→".dimmed(), suggestion.cyan());
        }
    }

    println!();
    println!(
        "Summary: {} passed, {} warnings, {} failed",
        report.pass_count.to_string().green(),
        report.warn_count.to_string().yellow(),
        report.fail_count.to_string().red()
    );

    if report.is_healthy() {
        println!();
        println!("{}", "Environment is healthy!".green().bold());
    } else {
        println!();
        println!(
            "{}",
            "Environment has issues that need attention.".red().bold()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::project::DATA_DIR_ENV_VAR;
    use serial_test::serial;
    use std::env;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn test_doctor_healthy_environment() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        // Set up environment
        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
            env::set_var("BOTBUS_AGENT", "test-agent");
        }

        // Create required directories
        let channels = temp.path().join("channels");
        fs::create_dir_all(&channels).unwrap();

        let mut report = DoctorReport::new();
        check_data_dir(&mut report);
        check_agent_identity(&mut report);

        // Should have 2 passes (data_dir and agent_identity)
        assert_eq!(report.pass_count, 2);
        assert_eq!(report.fail_count, 0);

        // Cleanup
        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
            env::remove_var("BOTBUS_AGENT");
        }
    }

    #[test]
    #[serial]
    fn test_doctor_missing_identity() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
            env::remove_var("BOTBUS_AGENT");
        }

        let mut report = DoctorReport::new();
        check_agent_identity(&mut report);

        assert_eq!(report.warn_count, 1);
        assert!(report.checks[0].suggestion.is_some());

        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
        }
    }
}
