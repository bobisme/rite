use anyhow::{Context, Result};
use chrono::Utc;
use colored::Colorize;
use globset::{Glob, GlobSetBuilder};
use std::path::Path;

use crate::core::claim::FileClaim;
use crate::core::message::{Message, MessageMeta};
use crate::core::project::{channel_path, claims_path, state_path};
use crate::storage::jsonl::{append_record, read_records};
use crate::storage::state::ProjectState;

pub struct ClaimOptions {
    pub patterns: Vec<String>,
    pub ttl: u64,
    pub message: Option<String>,
}

/// Claim files for editing.
pub fn claim(options: ClaimOptions, project_root: &Path) -> Result<()> {
    let state = ProjectState::new(state_path(project_root));
    let agent_name = state
        .current_agent()?
        .ok_or_else(|| anyhow::anyhow!("No agent registered. Run 'botbus register' first."))?;

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
pub fn claims(show_all: bool, mine_only: bool, project_root: &Path) -> Result<()> {
    let state = ProjectState::new(state_path(project_root));
    let current_agent = state.current_agent()?.unwrap_or_default();

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
pub fn release(patterns: Vec<String>, release_all: bool, project_root: &Path) -> Result<()> {
    let state = ProjectState::new(state_path(project_root));
    let agent_name = state
        .current_agent()?
        .ok_or_else(|| anyhow::anyhow!("No agent registered. Run 'botbus register' first."))?;

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
    use crate::cli::{init, register};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        register::run(Some("Claimer".to_string()), None, temp.path()).unwrap();
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
            },
            temp.path(),
        )
        .unwrap();

        claims(false, false, temp.path()).unwrap();
    }

    #[test]
    fn test_claim_and_release() {
        let temp = setup();

        claim(
            ClaimOptions {
                patterns: vec!["*.toml".to_string()],
                ttl: 3600,
                message: Some("Updating deps".to_string()),
            },
            temp.path(),
        )
        .unwrap();

        release(vec![], true, temp.path()).unwrap();
    }

    #[test]
    fn test_patterns_overlap() {
        assert!(patterns_overlap("src/**/*.rs", "src/main.rs"));
        assert!(patterns_overlap("src/auth/**", "src/auth/login.rs"));
        assert!(!patterns_overlap("src/api/**", "tests/**"));
    }
}
