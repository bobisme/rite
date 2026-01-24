use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use globset::{Glob, GlobSetBuilder};
use serde::Serialize;
use std::path::Path;

use crate::core::claim::FileClaim;
use crate::core::identity::{format_export, resolve_agent};
use crate::core::message::{Message, MessageMeta};
use crate::core::project::{channel_path, claims_path};
use crate::storage::jsonl::{append_record, read_records};

pub struct ClaimOptions {
    pub patterns: Vec<String>,
    pub ttl: u64,
    pub message: Option<String>,
    pub agent: Option<String>,
}

/// Claim files for editing.
pub fn claim(options: ClaimOptions, project_root: &Path) -> Result<()> {
    let agent_name = resolve_agent(options.agent.as_deref(), project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "No agent identity configured.\n\n\
             Set your identity with: {}\n\
             Or use --agent flag.",
            format_export("YourAgentName")
        )
    })?;

    // Load existing claims
    let claims: Vec<FileClaim> = read_records(&claims_path(project_root))?;

    // Check for conflicts
    let conflicts = check_conflicts(&options.patterns, &claims, &agent_name);
    if !conflicts.is_empty() {
        println!("{}", "Warning: Potential conflicts detected:".yellow());
        for (pattern, holder, expires) in &conflicts {
            let remaining = (*expires - Utc::now()).num_minutes();
            println!(
                "  {} overlaps with {}'s claim (expires in {}m)",
                pattern.cyan(),
                holder.yellow(),
                remaining
            );
        }
        println!();
    }

    // Create the claim
    let claim = FileClaim::new(&agent_name, options.patterns.clone(), options.ttl);

    // Append to claims.jsonl
    append_record(&claims_path(project_root), &claim).with_context(|| "Failed to record claim")?;

    // Post message to #general
    let body = if let Some(msg) = &options.message {
        format!("Claimed {} ({})", options.patterns.join(", "), msg)
    } else {
        format!("Claimed {}", options.patterns.join(", "))
    };

    let claim_msg = Message::new(&agent_name, "general", &body).with_meta(MessageMeta::Claim {
        patterns: options.patterns.clone(),
        ttl_secs: options.ttl,
    });

    append_record(&channel_path(project_root, "general"), &claim_msg)?;

    // Output
    println!(
        "{} Claimed {} pattern(s) for {}",
        "Success:".green(),
        options.patterns.len(),
        format_duration(options.ttl)
    );
    for pattern in &options.patterns {
        println!("  {}", pattern.cyan());
    }

    Ok(())
}

/// List active file claims.
pub fn claims(
    show_all: bool,
    mine_only: bool,
    agent: Option<&str>,
    project_root: &Path,
) -> Result<()> {
    let current_agent = resolve_agent(agent, project_root).unwrap_or_default();

    let all_claims: Vec<FileClaim> = read_records(&claims_path(project_root))?;

    // Build active claims map (latest state per claim ID)
    let mut active: std::collections::HashMap<ulid::Ulid, FileClaim> =
        std::collections::HashMap::new();
    for claim in all_claims {
        active.insert(claim.id, claim);
    }

    // Filter and sort
    let now = Utc::now();
    let mut claims_list: Vec<_> = active
        .values()
        .filter(|c| {
            if mine_only && c.agent != current_agent {
                return false;
            }
            if !show_all && (!c.active || c.expires_at < now) {
                return false;
            }
            true
        })
        .collect();

    claims_list.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));

    if claims_list.is_empty() {
        println!("No active claims.");
        return Ok(());
    }

    println!("{}", "Active Claims:".bold());
    for claim in &claims_list {
        let remaining = (claim.expires_at - now).num_minutes();
        let status = if claim.expires_at < now {
            "expired".red()
        } else if !claim.active {
            "released".dimmed()
        } else {
            format!("expires in {}m", remaining).green()
        };

        let agent = if claim.agent == current_agent {
            claim.agent.cyan().bold()
        } else {
            claim.agent.yellow().normal()
        };

        println!(
            "  {:<16} {:<30} {}",
            agent,
            claim.patterns.join(", ").dimmed(),
            status
        );
    }

    Ok(())
}

/// Release file claims.
pub fn release(
    patterns: Vec<String>,
    release_all: bool,
    agent: Option<&str>,
    project_root: &Path,
) -> Result<()> {
    let agent_name = resolve_agent(agent, project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "No agent identity configured.\n\n\
             Set your identity with: {}\n\
             Or use --agent flag.",
            format_export("YourAgentName")
        )
    })?;

    let all_claims: Vec<FileClaim> = read_records(&claims_path(project_root))?;

    // Build active claims map
    let mut active: std::collections::HashMap<ulid::Ulid, FileClaim> =
        std::collections::HashMap::new();
    for claim in all_claims {
        active.insert(claim.id, claim);
    }

    let now = Utc::now();
    let mut released_count = 0;

    for claim in active.values() {
        // Only release our own claims
        if claim.agent != agent_name {
            continue;
        }

        // Skip inactive or expired
        if !claim.active || claim.expires_at < now {
            continue;
        }

        // Check if we should release this one
        let should_release = if release_all {
            true
        } else if patterns.is_empty() {
            true
        } else {
            // Check if any pattern matches
            patterns.iter().any(|p| claim.patterns.contains(p))
        };

        if should_release {
            let release_record = claim.release();
            append_record(&claims_path(project_root), &release_record)?;
            released_count += 1;

            // Post release message
            let msg = Message::new(
                &agent_name,
                "general",
                format!("Released {}", claim.patterns.join(", ")),
            )
            .with_meta(MessageMeta::Release {
                patterns: claim.patterns.clone(),
            });
            append_record(&channel_path(project_root, "general"), &msg)?;
        }
    }

    if released_count == 0 {
        println!("No claims to release.");
    } else {
        println!(
            "{} Released {} claim(s)",
            "Success:".green(),
            released_count
        );
    }

    Ok(())
}

/// Output from check-claim command.
#[derive(Debug, Serialize)]
pub struct CheckClaimOutput {
    /// The file/pattern that was checked
    pub path: String,
    /// Whether the path is safe to edit (no conflicts)
    pub safe: bool,
    /// Conflicting claims (if any)
    pub conflicts: Vec<ClaimConflict>,
}

#[derive(Debug, Serialize)]
pub struct ClaimConflict {
    /// Agent who holds the claim
    pub agent: String,
    /// The pattern that matches
    pub pattern: String,
    /// When the claim expires
    pub expires_at: DateTime<Utc>,
    /// Seconds until expiration
    pub expires_in_secs: i64,
}

/// Check if a file/pattern conflicts with existing claims.
/// Returns Ok(true) if safe, Ok(false) if conflict exists.
/// Exits with code 1 on conflict for easy shell scripting.
pub fn check_claim(
    path: String,
    json: bool,
    agent: Option<&str>,
    project_root: &Path,
) -> Result<bool> {
    let current_agent = resolve_agent(agent, project_root).unwrap_or_default();
    let now = Utc::now();

    let all_claims: Vec<FileClaim> = read_records(&claims_path(project_root)).unwrap_or_default();

    // Build active claims map
    let mut active: std::collections::HashMap<ulid::Ulid, FileClaim> =
        std::collections::HashMap::new();
    for claim in all_claims {
        active.insert(claim.id, claim);
    }

    // Find conflicts
    let mut conflicts = Vec::new();
    for claim in active.values() {
        // Skip our own claims
        if claim.agent == current_agent {
            continue;
        }

        // Skip inactive or expired
        if !claim.active || claim.expires_at < now {
            continue;
        }

        // Check if our path matches any of their patterns
        for pattern in &claim.patterns {
            if path_matches_pattern(&path, pattern) {
                conflicts.push(ClaimConflict {
                    agent: claim.agent.clone(),
                    pattern: pattern.clone(),
                    expires_at: claim.expires_at,
                    expires_in_secs: (claim.expires_at - now).num_seconds(),
                });
            }
        }
    }

    let safe = conflicts.is_empty();

    let output = CheckClaimOutput {
        path: path.clone(),
        safe,
        conflicts,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if safe {
        println!("{} {} is safe to edit", "✓".green(), path.cyan());
    } else {
        println!("{} {} has conflicts:", "✗".red(), path.cyan());
        for conflict in &output.conflicts {
            let expires = format_duration(conflict.expires_in_secs as u64);
            println!(
                "  {} claimed {} (expires in {})",
                conflict.agent.yellow(),
                conflict.pattern.dimmed(),
                expires
            );
        }
    }

    Ok(safe)
}

/// Check if a specific path matches a glob pattern.
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    // Try glob matching
    if let Ok(glob) = Glob::new(pattern) {
        let mut builder = GlobSetBuilder::new();
        builder.add(glob);
        if let Ok(set) = builder.build() {
            if set.is_match(path) {
                return true;
            }
        }
    }

    // Also check prefix matching for directory patterns
    let pattern_base = pattern
        .split("**")
        .next()
        .unwrap_or(pattern)
        .trim_end_matches('/');
    if !pattern_base.is_empty() && path.starts_with(pattern_base) {
        return true;
    }

    false
}

fn check_conflicts(
    new_patterns: &[String],
    claims: &[FileClaim],
    my_agent: &str,
) -> Vec<(String, String, chrono::DateTime<Utc>)> {
    let mut conflicts = Vec::new();
    let now = Utc::now();

    // Build active claims map
    let mut active: std::collections::HashMap<ulid::Ulid, &FileClaim> =
        std::collections::HashMap::new();
    for claim in claims {
        active.insert(claim.id, claim);
    }

    for claim in active.values() {
        // Skip our own claims
        if claim.agent == my_agent {
            continue;
        }

        // Skip inactive or expired
        if !claim.active || claim.expires_at < now {
            continue;
        }

        // Check for overlaps
        for new_pat in new_patterns {
            for existing_pat in &claim.patterns {
                if patterns_overlap(new_pat, existing_pat) {
                    conflicts.push((new_pat.clone(), claim.agent.clone(), claim.expires_at));
                }
            }
        }
    }

    conflicts
}

fn patterns_overlap(a: &str, b: &str) -> bool {
    // Build glob matchers
    let mut builder_a = GlobSetBuilder::new();
    let mut builder_b = GlobSetBuilder::new();

    if let Ok(glob) = Glob::new(a) {
        builder_a.add(glob);
    }
    if let Ok(glob) = Glob::new(b) {
        builder_b.add(glob);
    }

    let set_a = builder_a.build().ok();
    let set_b = builder_b.build().ok();

    // Simple heuristic: if either pattern matches the other as a path, they overlap
    // This is not perfect but handles common cases

    // Check if b matches a as a literal path
    if let Some(ref set) = set_b {
        if set.is_match(a) {
            return true;
        }
    }

    // Check if a matches b as a literal path
    if let Some(ref set) = set_a {
        if set.is_match(b) {
            return true;
        }
    }

    // Check for common prefix (simple heuristic)
    let a_base = a.split("**").next().unwrap_or(a).trim_end_matches('/');
    let b_base = b.split("**").next().unwrap_or(b).trim_end_matches('/');

    if !a_base.is_empty() && !b_base.is_empty() {
        if a_base.starts_with(b_base) || b_base.starts_with(a_base) {
            return true;
        }
    }

    false
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::init;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_claim_and_list() {
        let temp = setup();

        claim(
            ClaimOptions {
                patterns: vec!["src/**/*.rs".to_string()],
                ttl: 3600,
                message: None,
                agent: Some("Claimer".to_string()),
            },
            temp.path(),
        )
        .unwrap();

        claims(false, false, Some("Claimer"), temp.path()).unwrap();
    }

    #[test]
    fn test_claim_and_release() {
        let temp = setup();

        claim(
            ClaimOptions {
                patterns: vec!["*.toml".to_string()],
                ttl: 3600,
                message: Some("Updating deps".to_string()),
                agent: Some("Claimer".to_string()),
            },
            temp.path(),
        )
        .unwrap();

        release(vec![], true, Some("Claimer"), temp.path()).unwrap();
    }

    #[test]
    fn test_patterns_overlap() {
        assert!(patterns_overlap("src/**/*.rs", "src/main.rs"));
        assert!(patterns_overlap("src/auth/**", "src/auth/login.rs"));
        assert!(!patterns_overlap("src/api/**", "tests/**"));
    }

    #[test]
    fn test_check_claim_no_conflicts() {
        let temp = setup();

        // No claims exist, should be safe
        let safe = check_claim(
            "src/main.rs".to_string(),
            false,
            Some("Checker"),
            temp.path(),
        )
        .unwrap();
        assert!(safe);
    }

    #[test]
    fn test_check_claim_with_conflict() {
        let temp = setup();

        // Another agent claims src/**
        claim(
            ClaimOptions {
                patterns: vec!["src/**".to_string()],
                ttl: 3600,
                message: None,
                agent: Some("OtherAgent".to_string()),
            },
            temp.path(),
        )
        .unwrap();

        // Checker tries to edit src/main.rs - should conflict
        let safe = check_claim(
            "src/main.rs".to_string(),
            false,
            Some("Checker"),
            temp.path(),
        )
        .unwrap();
        assert!(!safe);
    }

    #[test]
    fn test_check_claim_own_claim_ok() {
        let temp = setup();

        // Agent claims src/**
        claim(
            ClaimOptions {
                patterns: vec!["src/**".to_string()],
                ttl: 3600,
                message: None,
                agent: Some("MyAgent".to_string()),
            },
            temp.path(),
        )
        .unwrap();

        // Same agent checks - should be safe (own claim)
        let safe = check_claim(
            "src/main.rs".to_string(),
            false,
            Some("MyAgent"),
            temp.path(),
        )
        .unwrap();
        assert!(safe);
    }

    #[test]
    fn test_check_claim_json_output() {
        let temp = setup();

        claim(
            ClaimOptions {
                patterns: vec!["src/**".to_string()],
                ttl: 3600,
                message: None,
                agent: Some("OtherAgent".to_string()),
            },
            temp.path(),
        )
        .unwrap();

        // JSON output should work
        let safe = check_claim(
            "src/main.rs".to_string(),
            true, // json = true
            Some("Checker"),
            temp.path(),
        )
        .unwrap();
        assert!(!safe);
    }

    #[test]
    fn test_path_matches_pattern() {
        assert!(path_matches_pattern("src/main.rs", "src/**"));
        assert!(path_matches_pattern("src/auth/login.rs", "src/auth/**"));
        assert!(path_matches_pattern("Cargo.toml", "*.toml"));
        assert!(!path_matches_pattern("tests/foo.rs", "src/**"));
    }
}
