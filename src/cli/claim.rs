use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use globset::{Glob, GlobSetBuilder};
use serde::Serialize;
use std::path::Path;

use crate::core::claim::FileClaim;
use crate::core::identity::{require_agent, resolve_agent};
use crate::core::message::{Message, MessageMeta};
use crate::core::project::{channel_path, claims_path};
use crate::storage::jsonl::{append_record, read_records};

/// Expand a claim pattern to an absolute, canonicalized path.
/// Relative patterns are expanded from current working directory.
/// Glob wildcards are preserved.
///
/// # Security
/// Canonicalizes the base path portion to resolve symlinks and normalize
/// path components like `.` and `..`, preventing path confusion attacks.
fn expand_pattern(pattern: &str) -> String {
    // Get cwd for expansion
    let cwd = std::env::current_dir().unwrap_or_default();

    // Split into base (canonicalizable) and glob suffix
    let (base, suffix) = if let Some(idx) = pattern.find("**") {
        let base_end = pattern[..idx].rfind('/').map(|i| i + 1).unwrap_or(idx);
        (&pattern[..base_end], &pattern[base_end..])
    } else if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        // Simple glob - find last / before any glob char
        let glob_start = pattern
            .find(|c| c == '*' || c == '?' || c == '[')
            .unwrap_or(pattern.len());
        let base_end = pattern[..glob_start].rfind('/').map(|i| i + 1).unwrap_or(0);
        (&pattern[..base_end], &pattern[base_end..])
    } else {
        // No glob - entire path is base
        (pattern, "")
    };

    // Build absolute base path
    let abs_base = if base.starts_with('/') {
        Path::new(base).to_path_buf()
    } else if base.is_empty() {
        cwd.clone()
    } else {
        cwd.join(base)
    };

    // Try to canonicalize the base path (resolve symlinks, normalize ..)
    // If it fails (e.g., path doesn't exist yet), just use the cleaned absolute path
    let canonical_base = abs_base.canonicalize().unwrap_or_else(|_| abs_base.clone());

    // Recombine with glob suffix
    if suffix.is_empty() {
        canonical_base.to_string_lossy().to_string()
    } else {
        let base_str = canonical_base.to_string_lossy();
        let base_str = base_str.trim_end_matches('/');
        let suffix_str = suffix.trim_start_matches('/');
        format!("{}/{}", base_str, suffix_str)
    }
}

/// Display a claim pattern, making it relative if we're in the same tree.
fn display_pattern(pattern: &str) -> String {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => return pattern.to_string(),
    };

    // Extract the non-glob base path for comparison
    let base = pattern
        .split("**")
        .next()
        .unwrap_or(pattern)
        .trim_end_matches('/');

    // If base is empty (pattern starts with **), show as-is
    if base.is_empty() {
        return pattern.to_string();
    }

    let base_path = Path::new(base);

    // Check if the pattern base starts with cwd
    if let Ok(rel) = base_path.strip_prefix(&cwd) {
        // Reconstruct with relative base
        let rel_base = rel.to_string_lossy();
        if pattern.contains("**") {
            let suffix = &pattern[base.len()..];
            if rel_base.is_empty() {
                // Pattern base IS cwd, so relative is just the suffix without leading /
                suffix.trim_start_matches('/').to_string()
            } else {
                format!("{}{}", rel_base, suffix)
            }
        } else {
            rel_base.to_string()
        }
    } else {
        // Not in same tree, show absolute
        pattern.to_string()
    }
}

pub struct ClaimOptions {
    pub patterns: Vec<String>,
    pub ttl: u64,
    pub message: Option<String>,
    /// Extend TTL on existing claims matching this pattern
    pub extend: Option<String>,
    pub agent: Option<String>,
}

/// Claim files for editing.
pub fn claim(options: ClaimOptions) -> Result<()> {
    let agent_name = require_agent(options.agent.as_deref())?;

    // Handle --extend option: extend TTL on existing claims
    if let Some(extend_pattern) = &options.extend {
        let expanded = expand_pattern(extend_pattern);
        return extend_claims(&expanded, options.ttl, &agent_name);
    }

    // Expand patterns to absolute paths
    let expanded_patterns: Vec<String> =
        options.patterns.iter().map(|p| expand_pattern(p)).collect();

    // Load existing claims
    let claims: Vec<FileClaim> = read_records(&claims_path())?;

    // Check for conflicts - deny overlapping claims (using expanded patterns)
    let conflicts = check_conflicts(&expanded_patterns, &claims, &agent_name);
    if !conflicts.is_empty() {
        eprintln!("{}", "Error: Conflict with existing claim(s)".red().bold());
        for (pattern, holder, expires) in &conflicts {
            let remaining = (*expires - Utc::now()).num_minutes();
            eprintln!(
                "  {} owns {} (expires in {}m)",
                holder.yellow(),
                pattern.cyan(),
                remaining
            );
        }
        eprintln!();
        eprintln!("Ask them to release or narrow their claim:");
        // Get the first conflicting agent for the example
        let first_holder = &conflicts[0].1;
        eprintln!(
            "  {}",
            format!(
                "botbus send @{} \"Can you release {}? I need to work on it\"",
                first_holder,
                options.patterns.join(", ")
            )
            .dimmed()
        );
        anyhow::bail!("Cannot claim - conflicts with existing claims");
    }

    // Create the claim with expanded (absolute) patterns
    let claim = FileClaim::new(&agent_name, expanded_patterns.clone(), options.ttl);

    // Append to claims.jsonl
    append_record(&claims_path(), &claim).with_context(|| "Failed to record claim")?;

    // Post message to #general (use original patterns for readability)
    let display_patterns: Vec<String> = options.patterns.clone();
    let body = if let Some(msg) = &options.message {
        format!("Claimed {} ({})", display_patterns.join(", "), msg)
    } else {
        format!("Claimed {}", display_patterns.join(", "))
    };

    let claim_msg = Message::new(&agent_name, "general", &body).with_meta(MessageMeta::Claim {
        patterns: display_patterns.clone(),
        ttl_secs: options.ttl,
    });

    append_record(&channel_path("general"), &claim_msg)?;

    // Output (use original patterns for readability)
    println!(
        "{} Claimed {} pattern(s) for {}",
        "Success:".green(),
        display_patterns.len(),
        format_duration(options.ttl)
    );
    for pattern in &display_patterns {
        println!("  {}", pattern.cyan());
    }

    Ok(())
}

/// Extend TTL on existing claims matching the given pattern.
fn extend_claims(pattern: &str, ttl: u64, agent_name: &str) -> Result<()> {
    let all_claims: Vec<FileClaim> = read_records(&claims_path())?;
    let now = Utc::now();

    // Build active claims map
    let mut active: std::collections::HashMap<ulid::Ulid, FileClaim> =
        std::collections::HashMap::new();
    for claim in all_claims {
        active.insert(claim.id, claim);
    }

    let mut extended_count = 0;

    for claim in active.values() {
        // Only extend our own claims
        if claim.agent != agent_name {
            continue;
        }

        // Skip inactive or expired
        if !claim.active || claim.expires_at < now {
            continue;
        }

        // Check if any pattern matches
        let matches = claim.patterns.iter().any(|p| {
            p == pattern
                || p.contains(pattern)
                || pattern.contains(p)
                || patterns_overlap(p, pattern)
        });

        if matches {
            // Create extended claim (new record with same ID but new expiry)
            let extended = claim.extend(ttl);
            append_record(&claims_path(), &extended)?;
            extended_count += 1;

            // Post message
            let msg = Message::new(
                agent_name,
                "general",
                format!(
                    "Extended claim {} for {}",
                    claim.patterns.join(", "),
                    format_duration(ttl)
                ),
            );
            append_record(&channel_path("general"), &msg)?;
        }
    }

    if extended_count == 0 {
        println!("No matching claims to extend.");
    } else {
        println!(
            "{} Extended {} claim(s) for {}",
            "Success:".green(),
            extended_count,
            format_duration(ttl)
        );
    }

    Ok(())
}

/// Output for claims list.
#[derive(Debug, Serialize)]
pub struct ClaimsOutput {
    pub claims: Vec<ClaimInfo>,
}

#[derive(Debug, Serialize)]
pub struct ClaimInfo {
    pub agent: String,
    pub patterns: Vec<String>,
    pub active: bool,
    pub expires_at: DateTime<Utc>,
    pub expires_in_secs: i64,
}

/// List active file claims.
pub fn claims(json: bool, show_all: bool, mine_only: bool, agent: Option<&str>) -> Result<()> {
    let current_agent = resolve_agent(agent).unwrap_or_default();

    let all_claims: Vec<FileClaim> = read_records(&claims_path())?;

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

    if json {
        let output = ClaimsOutput {
            claims: claims_list
                .iter()
                .map(|c| ClaimInfo {
                    agent: c.agent.clone(),
                    patterns: c.patterns.clone(),
                    active: c.active,
                    expires_at: c.expires_at,
                    expires_in_secs: (c.expires_at - now).num_seconds(),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

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

        let agent_display = if claim.agent == current_agent {
            claim.agent.cyan().bold()
        } else {
            claim.agent.yellow().normal()
        };

        // Display patterns relative to cwd when possible
        let display_patterns: Vec<String> =
            claim.patterns.iter().map(|p| display_pattern(p)).collect();

        println!(
            "  {:<16} {:<30} {}",
            agent_display,
            display_patterns.join(", ").dimmed(),
            status
        );
    }

    Ok(())
}

/// Release file claims.
pub fn release(patterns: Vec<String>, release_all: bool, agent: Option<&str>) -> Result<()> {
    let agent_name = require_agent(agent)?;

    // Expand release patterns to absolute paths
    let expanded_patterns: Vec<String> = patterns.iter().map(|p| expand_pattern(p)).collect();

    let all_claims: Vec<FileClaim> = read_records(&claims_path())?;

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
        } else if expanded_patterns.is_empty() {
            true
        } else {
            // Check if any expanded pattern matches stored patterns
            expanded_patterns.iter().any(|p| claim.patterns.contains(p))
        };

        if should_release {
            let release_record = claim.release();
            append_record(&claims_path(), &release_record)?;
            released_count += 1;

            // Post release message (use display patterns for readability)
            let display_patterns: Vec<String> =
                claim.patterns.iter().map(|p| display_pattern(p)).collect();
            let msg = Message::new(
                &agent_name,
                "general",
                format!("Released {}", display_patterns.join(", ")),
            )
            .with_meta(MessageMeta::Release {
                patterns: display_patterns,
            });
            append_record(&channel_path("general"), &msg)?;
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
pub fn check_claim(path: String, json: bool, agent: Option<&str>) -> Result<bool> {
    let current_agent = resolve_agent(agent).unwrap_or_default();
    let now = Utc::now();

    // Expand path to absolute for matching against stored claims
    let expanded_path = expand_pattern(&path);

    let all_claims: Vec<FileClaim> = read_records(&claims_path()).unwrap_or_default();

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
            if path_matches_pattern(&expanded_path, pattern) {
                conflicts.push(ClaimConflict {
                    agent: claim.agent.clone(),
                    pattern: display_pattern(pattern), // Show relative when possible
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

    #[test]
    fn test_patterns_overlap() {
        assert!(patterns_overlap("src/**/*.rs", "src/main.rs"));
        assert!(patterns_overlap("src/auth/**", "src/auth/login.rs"));
        assert!(!patterns_overlap("src/api/**", "tests/**"));
    }

    #[test]
    fn test_path_matches_pattern() {
        assert!(path_matches_pattern("src/main.rs", "src/**"));
        assert!(path_matches_pattern("src/auth/login.rs", "src/auth/**"));
        assert!(path_matches_pattern("Cargo.toml", "*.toml"));
        assert!(!path_matches_pattern("tests/foo.rs", "src/**"));
    }

    #[test]
    fn test_expand_pattern() {
        // Absolute paths pass through unchanged
        assert_eq!(expand_pattern("/home/user/src/**"), "/home/user/src/**");

        // Relative paths get expanded (depends on cwd, so just check it's absolute)
        let expanded = expand_pattern("src/**");
        assert!(
            expanded.starts_with('/'),
            "Should be absolute: {}",
            expanded
        );
        assert!(
            expanded.ends_with("src/**"),
            "Should preserve glob: {}",
            expanded
        );
    }

    #[test]
    fn test_display_pattern() {
        // Patterns not in current tree are returned as-is
        let unrelated = display_pattern("/some/other/path/**");
        assert_eq!(unrelated, "/some/other/path/**");
    }

    // Integration tests that need project setup are moved to tests/integration/
    // since they require the global data directory to be mocked
}
