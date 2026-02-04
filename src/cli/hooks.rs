//! Channel hooks — trigger commands when messages are sent to channels.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use super::OutputFormat;
use super::format::{to_toon, to_toon_list};
use crate::core::claim::FileClaim;
use crate::core::hook::{ClaimRelease, Hook, HookCondition, HookFiring, shell_display};
use crate::core::message::{Message, MessageMeta, SystemEvent};
use crate::core::project::{channel_path, claims_path, hooks_audit_path, hooks_path};
use crate::storage::jsonl::{append_if, append_record, read_records};

/// Parse a cooldown duration string (e.g., "30s", "5m", "1h").
/// Returns seconds. Defaults to seconds if no unit.
fn parse_cooldown(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        bail!("Empty cooldown value");
    }

    let last = s.chars().last().unwrap();
    if last.is_ascii_digit() {
        // No unit — assume seconds
        return s.parse::<u64>().context("Invalid cooldown number");
    }

    let number_part = &s[..s.len() - 1];
    let value: u64 = number_part.parse().context("Invalid cooldown number")?;

    match last {
        's' => Ok(value),
        'm' => Ok(value * 60),
        'h' => Ok(value * 3600),
        _ => bail!("Unknown cooldown unit '{}'. Use s, m, or h.", last),
    }
}

/// Add a new hook.
#[allow(clippy::too_many_arguments)]
pub fn add(
    channel: Option<String>,
    claim: Option<String>,
    mention: Option<String>,
    cwd: PathBuf,
    cooldown: Option<String>,
    command: Vec<String>,
    ttl: Option<u64>,
    release_on_exit: bool,
    claim_owner: Option<String>,
    agent: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    if command.is_empty() {
        bail!(
            "Command is required. Use -- before the command, e.g.:\n  bus hooks add --channel ch --claim pattern --cwd /tmp --release-on-exit -- echo hello"
        );
    }

    // Determine which condition type to use
    let condition = match (claim.as_ref(), mention.as_ref()) {
        (Some(pattern), None) => {
            // Claim-based hooks require explicit channel
            if channel.is_none() {
                bail!("Claim-based hooks require --channel to be specified");
            }
            HookCondition::ClaimAvailable {
                pattern: pattern.clone(),
            }
        }
        (None, Some(agent_name)) => HookCondition::MentionReceived {
            agent: agent_name
                .strip_prefix('@')
                .unwrap_or(agent_name)
                .to_string(),
        },
        (None, None) => bail!("Must specify either --claim or --mention"),
        (Some(_), Some(_)) => bail!("Cannot specify both --claim and --mention"),
    };

    // Default channel to "*" (all non-DM channels) if not specified
    let hook_channel = channel.unwrap_or_else(|| "*".to_string());

    // Validate claim release strategy (required for ClaimAvailable hooks, optional for MentionReceived)
    if matches!(condition, HookCondition::ClaimAvailable { .. })
        && ttl.is_none()
        && !release_on_exit
    {
        bail!("Must specify either --ttl <seconds> or --release-on-exit for claim acquisition");
    }

    // Validate cwd exists and is a directory
    if !cwd.exists() {
        bail!("Working directory does not exist: {}", cwd.display());
    }
    if !cwd.is_dir() {
        bail!("Working directory is not a directory: {}", cwd.display());
    }

    let cooldown_secs = match cooldown {
        Some(ref s) => parse_cooldown(s)?,
        None => 30,
    };

    // Load existing hooks to check for ID collisions
    let existing_hooks: Vec<Hook> = read_records(&hooks_path()).unwrap_or_default();
    let existing_ids: Vec<String> = build_active_hooks(&existing_hooks)
        .values()
        .map(|h| h.id.clone())
        .collect();

    // Set claim_release for ClaimAvailable hooks (required) or MentionReceived hooks with ttl (optional)
    let claim_release = if matches!(condition, HookCondition::ClaimAvailable { .. }) {
        // ClaimAvailable hooks always need claim_release
        if let Some(secs) = ttl {
            Some(ClaimRelease::Ttl { secs })
        } else {
            Some(ClaimRelease::OnExit)
        }
    } else if ttl.is_some() || release_on_exit {
        // MentionReceived hooks can optionally have claim_release to prevent duplicate spawns
        if let Some(secs) = ttl {
            Some(ClaimRelease::Ttl { secs })
        } else {
            Some(ClaimRelease::OnExit)
        }
    } else {
        None
    };

    let hook = Hook {
        id: Hook::generate_id(&existing_ids),
        channel: hook_channel.clone(),
        condition,
        command,
        cwd,
        cooldown_secs,
        last_fired: None,
        created_at: Utc::now(),
        created_by: agent.map(|s| s.to_string()),
        claim_release,
        claim_owner,
        active: true,
    };

    append_record(&hooks_path(), &hook).context("Failed to save hook")?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&hook)?);
        }
        OutputFormat::Toon => {
            println!("{}", to_toon(&hook));
        }
        OutputFormat::Text => {
            println!("{} Hook {} created", "Added:".green(), hook.id.cyan());
            println!("  channel: #{}", hook.channel);
            println!("  condition: {:?}", hook.condition);
            println!("  command: {:?}", hook.command);
            println!("  cooldown: {}s", hook.cooldown_secs);
        }
    }

    Ok(())
}

/// Output struct for hook listing.
#[derive(Debug, Serialize)]
struct HookInfo {
    id: String,
    channel: String,
    condition: HookCondition,
    command: Vec<String>,
    cwd: String,
    cooldown_secs: u64,
    last_fired: Option<String>,
    active: bool,
}

/// List all active hooks.
pub fn list(format: OutputFormat) -> Result<()> {
    let all_hooks: Vec<Hook> = read_records(&hooks_path()).unwrap_or_default();
    let active = build_active_hooks(&all_hooks);

    let mut hooks: Vec<&Hook> = active.values().collect();
    hooks.sort_by_key(|h| &h.created_at);

    let infos: Vec<HookInfo> = hooks
        .iter()
        .map(|h| HookInfo {
            id: h.id.clone(),
            channel: h.channel.clone(),
            condition: h.condition.clone(),
            command: h.command.clone(),
            cwd: h.cwd.to_string_lossy().to_string(),
            cooldown_secs: h.cooldown_secs,
            last_fired: h.last_fired.map(|t| t.to_rfc3339()),
            active: h.active,
        })
        .collect();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&infos)?);
        }
        OutputFormat::Toon => {
            if infos.is_empty() {
                println!("hooks: []");
            } else {
                println!("{}", to_toon_list(&infos));
            }
        }
        OutputFormat::Text => {
            if hooks.is_empty() {
                println!("No active hooks.");
            } else {
                println!("{}", "Hooks:".bold());
                for h in &hooks {
                    let last = h
                        .last_fired
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_else(|| "never".to_string());
                    println!(
                        "  {} #{} → {:?} (cooldown: {}s, last: {})",
                        h.id.cyan(),
                        h.channel,
                        h.command,
                        h.cooldown_secs,
                        last.dimmed()
                    );
                    match &h.condition {
                        HookCondition::ClaimAvailable { pattern } => {
                            println!("    if-claim-available: {}", pattern);
                        }
                        HookCondition::MentionReceived { agent } => {
                            println!("    if-mention-received: @{}", agent);
                        }
                    }
                    if let Some(ref owner) = h.claim_owner {
                        println!("    claim-owner: {}", owner);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Remove (deactivate) a hook by ID.
pub fn remove(hook_id: String, format: OutputFormat) -> Result<()> {
    let all_hooks: Vec<Hook> = read_records(&hooks_path()).unwrap_or_default();
    let active = build_active_hooks(&all_hooks);

    let hook = active
        .get(&hook_id)
        .ok_or_else(|| anyhow::anyhow!("Hook not found: {}", hook_id))?;

    // Append a deactivated copy
    let mut deactivated = hook.clone();
    deactivated.active = false;

    append_record(&hooks_path(), &deactivated).context("Failed to deactivate hook")?;

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "removed": hook_id
                }))?
            );
        }
        OutputFormat::Toon => {
            println!("removed: {}", hook_id);
        }
        OutputFormat::Text => {
            println!("{} Hook {} removed", "Removed:".green(), hook_id.cyan());
        }
    }

    Ok(())
}

/// Dry-run test of a hook — evaluate condition without executing.
pub fn test(hook_id: String, format: OutputFormat) -> Result<()> {
    let all_hooks: Vec<Hook> = read_records(&hooks_path()).unwrap_or_default();
    let active = build_active_hooks(&all_hooks);

    let hook = active
        .get(&hook_id)
        .ok_or_else(|| anyhow::anyhow!("Hook not found: {}", hook_id))?;

    let now = Utc::now();

    // Check cooldown
    let cooldown_ok = match hook.last_fired {
        Some(last) => (now - last).num_seconds() >= hook.cooldown_secs as i64,
        None => true,
    };

    // Evaluate condition (MentionReceived hooks will always return false in test mode)
    let condition_result = evaluate_condition(&hook.condition, &[])?;

    let would_execute = cooldown_ok && condition_result;

    let reason = if !cooldown_ok {
        Some("cooldown active".to_string())
    } else if !condition_result {
        Some("condition not met".to_string())
    } else {
        None
    };

    #[derive(Serialize)]
    struct TestResult {
        hook_id: String,
        cooldown_ok: bool,
        condition_result: bool,
        would_execute: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        command: Vec<String>,
        cwd: String,
    }

    let result = TestResult {
        hook_id: hook.id.clone(),
        cooldown_ok,
        condition_result,
        would_execute,
        reason,
        command: hook.command.clone(),
        cwd: hook.cwd.to_string_lossy().to_string(),
    };

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        OutputFormat::Toon => {
            println!("{}", to_toon(&result));
        }
        OutputFormat::Text => {
            println!("{} Hook {} dry-run:", "Test:".green(), hook.id.cyan());
            println!(
                "  cooldown: {}",
                if cooldown_ok {
                    "ok".green().to_string()
                } else {
                    "active (skipped)".red().to_string()
                }
            );
            println!(
                "  condition: {}",
                if condition_result {
                    "passed".green().to_string()
                } else {
                    "failed".red().to_string()
                }
            );
            println!(
                "  would execute: {}",
                if would_execute {
                    "yes".green().to_string()
                } else {
                    "no".red().to_string()
                }
            );
            println!("  command: {:?}", hook.command);
            println!("  cwd: {}", hook.cwd.display());
        }
    }

    Ok(())
}

/// Build a map of active hooks (latest state per ID wins).
fn build_active_hooks(all_hooks: &[Hook]) -> HashMap<String, Hook> {
    let mut map: HashMap<String, Hook> = HashMap::new();
    for hook in all_hooks {
        map.insert(hook.id.clone(), hook.clone());
    }
    // Remove inactive hooks
    map.retain(|_, h| h.active);
    map
}

/// Check if a claim pattern has NO active holder.
/// Returns true if the pattern is available (no one holds it).
fn is_claim_available(pattern: &str) -> Result<bool> {
    let all_claims: Vec<FileClaim> = read_records(&claims_path()).unwrap_or_default();
    let now = Utc::now();

    // Build active claims map (latest state per ID wins)
    let mut active: HashMap<ulid::Ulid, FileClaim> = HashMap::new();
    for claim in all_claims {
        active.insert(claim.id, claim);
    }

    // Check if ANY active, non-expired claim holds this exact pattern
    for claim in active.values() {
        if !claim.active || claim.expires_at < now {
            continue;
        }
        if claim.patterns.iter().any(|p| p == pattern) {
            return Ok(false); // Someone holds it
        }
    }

    Ok(true) // No one holds it — available
}

/// Evaluate a hook condition.
fn evaluate_condition(condition: &HookCondition, mentions: &[String]) -> Result<bool> {
    match condition {
        HookCondition::ClaimAvailable { pattern } => is_claim_available(pattern),
        HookCondition::MentionReceived { agent } => Ok(mentions.iter().any(|m| m == agent)),
    }
}

/// Result of a hook that fired during evaluation.
pub struct HookFireResult {
    pub hook_id: String,
    pub command_display: String,
    pub claim_pattern: Option<String>,
    pub claim_ttl: Option<u64>,
}

/// Evaluate all hooks for a channel after a message is sent.
/// Returns info about hooks that fired (for caller to display).
pub fn evaluate_hooks(
    channel: &str,
    message_id: &str,
    meta: Option<&MessageMeta>,
    agent: &str,
    mentions: &[String],
) -> Vec<HookFireResult> {
    match evaluate_hooks_inner(channel, message_id, meta, agent, mentions) {
        Ok(results) => results,
        Err(e) => {
            eprintln!("Warning: hook evaluation failed: {}", e);
            vec![]
        }
    }
}

fn evaluate_hooks_inner(
    channel: &str,
    message_id: &str,
    meta: Option<&MessageMeta>,
    agent: &str,
    mentions: &[String],
) -> Result<Vec<HookFireResult>> {
    // Skip hook evaluation for system messages to prevent recursive loops
    if matches!(meta, Some(MessageMeta::System { .. })) {
        return Ok(vec![]);
    }

    let all_hooks: Vec<Hook> = read_records(&hooks_path()).unwrap_or_default();
    if all_hooks.is_empty() {
        return Ok(vec![]);
    }

    let active = build_active_hooks(&all_hooks);
    let now = Utc::now();
    let mut results = Vec::new();

    for hook in active.values() {
        // Match hook channel: exact match OR wildcard "*" (except DMs)
        let channel_matches = if hook.channel == "*" {
            !crate::core::channel::is_dm_channel(channel)
        } else {
            hook.channel == channel
        };

        if !channel_matches {
            continue;
        }

        // Check cooldown
        let cooldown_ok = match hook.last_fired {
            Some(last) => (now - last).num_seconds() >= hook.cooldown_secs as i64,
            None => true,
        };

        if !cooldown_ok {
            let firing = HookFiring {
                ts: now,
                hook_id: hook.id.clone(),
                channel: channel.to_string(),
                message_id: message_id.to_string(),
                condition_result: false,
                executed: false,
                reason: Some("cooldown active".to_string()),
            };
            let _ = append_record(&hooks_audit_path(), &firing);
            continue;
        }

        // Handle condition evaluation and claim acquisition
        // For ClaimAvailable hooks, we do an atomic check-and-stake to prevent races
        let (claim, claim_ttl, claim_pattern) = match &hook.condition {
            HookCondition::ClaimAvailable { pattern } => {
                // Use claim_owner if specified, otherwise use message sender
                let claim_agent = hook.claim_owner.as_deref().unwrap_or(agent);
                let pattern_clone = pattern.clone();

                match &hook.claim_release {
                    Some(ClaimRelease::Ttl { secs }) => {
                        let ttl = *secs;
                        let c = FileClaim::new(claim_agent, vec![pattern.clone()], ttl);

                        // Atomic check-and-stake: only append if pattern is still available
                        let acquired = append_if(&claims_path(), &c, |existing_claims| {
                            let now = Utc::now();
                            // Check if ANY active, non-expired claim holds this pattern
                            !existing_claims.iter().any(|claim: &FileClaim| {
                                claim.active
                                    && claim.expires_at > now
                                    && claim.patterns.iter().any(|p| p == &pattern_clone)
                            })
                        })
                        .unwrap_or(false);

                        if !acquired {
                            let firing = HookFiring {
                                ts: now,
                                hook_id: hook.id.clone(),
                                channel: channel.to_string(),
                                message_id: message_id.to_string(),
                                condition_result: false,
                                executed: false,
                                reason: Some("claim unavailable (atomic check)".to_string()),
                            };
                            let _ = append_record(&hooks_audit_path(), &firing);
                            continue;
                        }

                        (Some(c), Some(ttl), Some(pattern.clone()))
                    }
                    Some(ClaimRelease::OnExit) => {
                        // Use large sentinel TTL; released explicitly after command exits
                        let c = FileClaim::new(claim_agent, vec![pattern.clone()], 86400);

                        // Atomic check-and-stake
                        let acquired = append_if(&claims_path(), &c, |existing_claims| {
                            let now = Utc::now();
                            !existing_claims.iter().any(|claim: &FileClaim| {
                                claim.active
                                    && claim.expires_at > now
                                    && claim.patterns.iter().any(|p| p == &pattern_clone)
                            })
                        })
                        .unwrap_or(false);

                        if !acquired {
                            let firing = HookFiring {
                                ts: now,
                                hook_id: hook.id.clone(),
                                channel: channel.to_string(),
                                message_id: message_id.to_string(),
                                condition_result: false,
                                executed: false,
                                reason: Some("claim unavailable (atomic check)".to_string()),
                            };
                            let _ = append_record(&hooks_audit_path(), &firing);
                            continue;
                        }

                        (Some(c), None, Some(pattern.clone()))
                    }
                    None => {
                        // No claim release strategy - just check availability without claiming
                        let condition_result = is_claim_available(pattern).unwrap_or(false);
                        if !condition_result {
                            let firing = HookFiring {
                                ts: now,
                                hook_id: hook.id.clone(),
                                channel: channel.to_string(),
                                message_id: message_id.to_string(),
                                condition_result: false,
                                executed: false,
                                reason: Some("condition not met".to_string()),
                            };
                            let _ = append_record(&hooks_audit_path(), &firing);
                            continue;
                        }
                        (None, None, Some(pattern.clone()))
                    }
                }
            }
            HookCondition::MentionReceived {
                agent: mention_agent,
            } => {
                // Check if agent is mentioned
                let condition_result = mentions.iter().any(|m| m == mention_agent);
                if !condition_result {
                    let firing = HookFiring {
                        ts: now,
                        hook_id: hook.id.clone(),
                        channel: channel.to_string(),
                        message_id: message_id.to_string(),
                        condition_result: false,
                        executed: false,
                        reason: Some("condition not met".to_string()),
                    };
                    let _ = append_record(&hooks_audit_path(), &firing);
                    continue;
                }

                // If hook has claim_release, acquire a claim to prevent duplicate spawns
                // Auto-derive pattern: respond://<agent>
                if let Some(ref release) = hook.claim_release {
                    let claim_pattern = format!("respond://{}", mention_agent);
                    let claim_agent = hook.claim_owner.as_deref().unwrap_or(agent);

                    match release {
                        ClaimRelease::Ttl { secs } => {
                            let ttl = *secs;
                            let c = FileClaim::new(claim_agent, vec![claim_pattern.clone()], ttl);
                            let pattern_for_check = claim_pattern.clone();

                            // Atomic check-and-stake
                            let acquired = append_if(&claims_path(), &c, |existing_claims| {
                                let now = Utc::now();
                                !existing_claims.iter().any(|claim: &FileClaim| {
                                    claim.active
                                        && claim.expires_at > now
                                        && claim.patterns.iter().any(|p| p == &pattern_for_check)
                                })
                            })
                            .unwrap_or(false);

                            if !acquired {
                                let firing = HookFiring {
                                    ts: now,
                                    hook_id: hook.id.clone(),
                                    channel: channel.to_string(),
                                    message_id: message_id.to_string(),
                                    condition_result: true,
                                    executed: false,
                                    reason: Some("claim unavailable (mention hook)".to_string()),
                                };
                                let _ = append_record(&hooks_audit_path(), &firing);
                                continue;
                            }

                            (Some(c), Some(ttl), Some(claim_pattern))
                        }
                        ClaimRelease::OnExit => {
                            // Use large sentinel TTL; released explicitly after command exits
                            let c = FileClaim::new(claim_agent, vec![claim_pattern.clone()], 86400);
                            let pattern_for_check = claim_pattern.clone();

                            let acquired = append_if(&claims_path(), &c, |existing_claims| {
                                let now = Utc::now();
                                !existing_claims.iter().any(|claim: &FileClaim| {
                                    claim.active
                                        && claim.expires_at > now
                                        && claim.patterns.iter().any(|p| p == &pattern_for_check)
                                })
                            })
                            .unwrap_or(false);

                            if !acquired {
                                let firing = HookFiring {
                                    ts: now,
                                    hook_id: hook.id.clone(),
                                    channel: channel.to_string(),
                                    message_id: message_id.to_string(),
                                    condition_result: true,
                                    executed: false,
                                    reason: Some("claim unavailable (mention hook)".to_string()),
                                };
                                let _ = append_record(&hooks_audit_path(), &firing);
                                continue;
                            }

                            (Some(c), None, Some(claim_pattern))
                        }
                    }
                } else {
                    // No claim needed for simple mention hooks
                    (None, None, None)
                }
            }
        };

        let is_on_exit = matches!(hook.claim_release, Some(ClaimRelease::OnExit));
        let cmd_display = shell_display(&hook.command);

        // Spawn the command
        let executed = if hook.command.is_empty() {
            if let Some(c) = &claim {
                let _ = append_record(&claims_path(), &c.release());
            }
            false
        } else {
            match std::process::Command::new(&hook.command[0])
                .args(&hook.command[1..])
                .current_dir(&hook.cwd)
                .env("BOTBUS_CHANNEL", channel)
                .env("BOTBUS_MESSAGE_ID", message_id)
                .env("BOTBUS_AGENT", agent)
                .env("BOTBUS_HOOK_ID", &hook.id)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    if is_on_exit {
                        // Block until command exits, then release claim
                        let _ = child.wait();
                        if let Some(c) = &claim {
                            let _ = append_record(&claims_path(), &c.release());
                        }
                    }
                    true
                }
                Err(_) => {
                    if let Some(c) = &claim {
                        let _ = append_record(&claims_path(), &c.release());
                    }
                    false
                }
            }
        };

        // Post system message to channel
        if executed {
            let sys_msg = Message::new(
                "system",
                channel,
                format!("Hook {} fired: {}", hook.id, cmd_display),
            )
            .with_meta(MessageMeta::System {
                event: SystemEvent::HookFired {
                    hook_id: hook.id.clone(),
                    command: hook.command.clone(),
                },
            });
            let _ = append_record(&channel_path(channel), &sys_msg);

            results.push(HookFireResult {
                hook_id: hook.id.clone(),
                command_display: cmd_display,
                claim_pattern,
                claim_ttl,
            });
        }

        // Update last_fired
        let mut updated = hook.clone();
        updated.last_fired = Some(now);
        let _ = append_record(&hooks_path(), &updated);

        // Audit log
        let firing = HookFiring {
            ts: now,
            hook_id: hook.id.clone(),
            channel: channel.to_string(),
            message_id: message_id.to_string(),
            condition_result: true,
            executed,
            reason: if executed {
                None
            } else {
                Some("spawn failed".to_string())
            },
        };
        let _ = append_record(&hooks_audit_path(), &firing);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cooldown() {
        assert_eq!(parse_cooldown("30s").unwrap(), 30);
        assert_eq!(parse_cooldown("5m").unwrap(), 300);
        assert_eq!(parse_cooldown("1h").unwrap(), 3600);
        assert_eq!(parse_cooldown("60").unwrap(), 60);
        assert!(parse_cooldown("").is_err());
        assert!(parse_cooldown("abc").is_err());
    }

    #[test]
    fn test_build_active_hooks() {
        let hooks = vec![
            Hook {
                id: "hk-abc".to_string(),
                channel: "test".to_string(),
                condition: HookCondition::ClaimAvailable {
                    pattern: "p".to_string(),
                },
                command: vec!["echo".to_string()],
                cwd: PathBuf::from("/tmp"),
                cooldown_secs: 30,
                last_fired: None,
                created_at: Utc::now(),
                created_by: None,
                claim_release: Some(ClaimRelease::OnExit),
                claim_owner: None,
                active: true,
            },
            Hook {
                id: "hk-abc".to_string(),
                channel: "test".to_string(),
                condition: HookCondition::ClaimAvailable {
                    pattern: "p".to_string(),
                },
                command: vec!["echo".to_string()],
                cwd: PathBuf::from("/tmp"),
                cooldown_secs: 30,
                last_fired: None,
                created_at: Utc::now(),
                created_by: None,
                claim_release: Some(ClaimRelease::OnExit),
                claim_owner: None,
                active: false, // Deactivated
            },
        ];

        let active = build_active_hooks(&hooks);
        assert!(active.is_empty()); // Second record deactivated it
    }

    #[test]
    fn test_is_claim_available_no_claims() {
        // With no claims file, everything is available
        // This test relies on the default data dir not having claims,
        // but since read_records returns empty on missing file, it works.
        // We'll test this more thoroughly in integration tests.
        let result = is_claim_available("agent://nonexistent-test-pattern-12345");
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
