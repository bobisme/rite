//! Channel hooks — trigger commands when messages are sent to channels.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, instrument, warn};

use super::OutputFormat;
use crate::core::claim::FileClaim;
use crate::core::flags::HookFlags;
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
    priority: i32,
    require_flag: Option<String>,
    description: Option<String>,
    agent: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    if command.is_empty() {
        bail!(
            "Command is required. Use -- before the command, e.g.:\n  rite hooks add --channel ch --claim pattern --cwd /tmp --release-on-exit -- echo hello"
        );
    }

    // Determine which condition type to use
    let condition = match (claim.as_ref(), mention.as_ref()) {
        (Some(pattern), None) => {
            // Claim-only hooks require explicit channel
            if channel.is_none() {
                bail!("Claim-based hooks require --channel to be specified");
            }
            HookCondition::ClaimAvailable {
                pattern: pattern.clone(),
            }
        }
        (Some(_), Some(agent_name)) => {
            // Mention + claim: fires on @mention, acquires claim atomically
            if channel.is_none() {
                bail!("Hooks with --claim require --channel to be specified");
            }
            // Condition is MentionReceived; claim pattern stored in hook.claim_pattern
            HookCondition::MentionReceived {
                agent: agent_name
                    .strip_prefix('@')
                    .unwrap_or(agent_name)
                    .to_string(),
            }
        }
        (None, Some(agent_name)) => HookCondition::MentionReceived {
            agent: agent_name
                .strip_prefix('@')
                .unwrap_or(agent_name)
                .to_string(),
        },
        (None, None) => bail!("Must specify either --claim or --mention"),
    };

    // Default channel to "*" (all non-DM channels) if not specified
    let hook_channel = channel.unwrap_or_else(|| "*".to_string());

    // Validate claim release strategy
    // Required for ClaimAvailable hooks; required when --claim is used with --mention
    let has_claim = claim.is_some();
    if has_claim && ttl.is_none() && !release_on_exit {
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

    // Set claim_release when --claim is used (required for claim hooks, optional otherwise)
    let claim_release = if has_claim {
        if let Some(secs) = ttl {
            Some(ClaimRelease::Ttl { secs })
        } else {
            Some(ClaimRelease::OnExit)
        }
    } else {
        None
    };

    // For mention+claim hooks, store the explicit claim pattern
    let claim_pattern = if matches!(condition, HookCondition::MentionReceived { .. }) {
        claim
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
        claim_pattern,
        claim_owner,
        priority,
        require_flag: require_flag.map(|f| f.to_lowercase()),
        active: true,
        description,
    };

    append_record(&hooks_path(), &hook).context("Failed to save hook")?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&hook)?);
        }
        OutputFormat::Pretty | OutputFormat::Text => {
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
    priority: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_flag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    last_fired: Option<String>,
    active: bool,
}

#[derive(Debug, Serialize)]
struct HooksOutput {
    hooks: Vec<HookInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advice: Vec<String>,
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
            priority: h.priority,
            require_flag: h.require_flag.clone(),
            description: h.description.clone(),
            last_fired: h.last_fired.map(|t| t.to_rfc3339()),
            active: h.active,
        })
        .collect();

    match format {
        OutputFormat::Json => {
            let output = HooksOutput {
                hooks: infos,
                advice: vec![], // Informational command, no specific next action
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Pretty => {
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
                        "  {} #{} → {:?} (priority: {}, cooldown: {}s, last: {})",
                        h.id.cyan(),
                        h.channel,
                        h.command,
                        h.priority,
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
                    if let Some(ref flag) = h.require_flag {
                        println!("    require-flag: !{}", flag);
                    }
                    if let Some(ref desc) = h.description {
                        println!("    description: {}", desc);
                    }
                }
            }
        }
        OutputFormat::Text => {
            for h in &hooks {
                let event = match &h.condition {
                    HookCondition::ClaimAvailable { .. } => "claim-available",
                    HookCondition::MentionReceived { .. } => "mention",
                };
                let command_str = shell_display(&h.command);
                let desc_str = h.description.as_deref().unwrap_or("");
                if desc_str.is_empty() {
                    println!("{}  {}  {}  {}", h.id, h.channel, event, command_str);
                } else {
                    println!(
                        "{}  {}  {}  {}  {}",
                        h.id, h.channel, event, command_str, desc_str
                    );
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
        OutputFormat::Pretty | OutputFormat::Text => {
            println!("{} Hook {} removed", "Removed:".green(), hook_id.cyan());
        }
    }

    Ok(())
}

/// Rename a channel in all hooks that reference it.
/// Returns the count of hooks that were updated.
pub fn rename_channel_in_hooks(old_name: &str, new_name: &str) -> Result<usize> {
    let all_hooks: Vec<Hook> = read_records(&hooks_path()).unwrap_or_default();

    // Find hooks that need updating (only active hooks with matching channel)
    let active = build_active_hooks(&all_hooks);
    let hooks_to_update: Vec<Hook> = active
        .values()
        .filter(|h| h.channel == old_name)
        .cloned()
        .collect();

    let update_count = hooks_to_update.len();

    // If no hooks need updating, return early
    if update_count == 0 {
        return Ok(0);
    }

    // Append updated versions with new channel name
    for mut hook in hooks_to_update {
        hook.channel = new_name.to_string();
        append_record(&hooks_path(), &hook).context("Failed to update hook")?;
    }

    Ok(update_count)
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
        OutputFormat::Pretty | OutputFormat::Text => {
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

/// Check if a pattern is held by any active claim in the given claims list.
/// This properly deduplicates claims by ID (latest state wins) before checking.
fn is_pattern_held(pattern: &str, existing_claims: &[FileClaim], now: DateTime<Utc>) -> bool {
    // Build active claims map (latest state per ID wins)
    let mut active: HashMap<ulid::Ulid, &FileClaim> = HashMap::new();
    for claim in existing_claims {
        active.insert(claim.id, claim);
    }

    // Check if ANY active, non-expired claim holds this exact pattern
    active.values().any(|claim| {
        claim.active && claim.expires_at > now && claim.patterns.iter().any(|p| p == pattern)
    })
}

/// Check if a claim pattern has NO active holder.
/// Returns true if the pattern is available (no one holds it).
fn is_claim_available(pattern: &str) -> Result<bool> {
    let all_claims: Vec<FileClaim> = read_records(&claims_path()).unwrap_or_default();
    let now = Utc::now();
    Ok(!is_pattern_held(pattern, &all_claims, now))
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
#[instrument(skip(meta, mentions), fields(channel = channel, message_id = message_id, agent = agent))]
pub fn evaluate_hooks(
    channel: &str,
    message_id: &str,
    meta: Option<&MessageMeta>,
    agent: &str,
    mentions: &[String],
) -> Vec<HookFireResult> {
    evaluate_hooks_with_flags(
        channel,
        message_id,
        meta,
        agent,
        mentions,
        &HookFlags::default(),
    )
}

/// Evaluate hooks with explicit flag control.
/// Flags can suppress channel hooks, mention hooks, or both.
#[instrument(skip(meta, mentions, flags), fields(channel = channel, message_id = message_id, agent = agent))]
pub fn evaluate_hooks_with_flags(
    channel: &str,
    message_id: &str,
    meta: Option<&MessageMeta>,
    agent: &str,
    mentions: &[String],
    flags: &HookFlags,
) -> Vec<HookFireResult> {
    match evaluate_hooks_inner(channel, message_id, meta, agent, mentions, flags) {
        Ok(results) => results,
        Err(error) => {
            warn!(%error, "hook evaluation failed");
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
    flags: &HookFlags,
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

    // Collect hooks into a vector and sort by priority (lower priority runs first)
    let mut hooks_to_process: Vec<&Hook> = active.values().collect();
    hooks_to_process.sort_by_key(|h| h.priority);

    for hook in hooks_to_process {
        // Check if hook is suppressed by flags
        let is_channel_hook = matches!(hook.condition, HookCondition::ClaimAvailable { .. });
        let is_mention_hook = matches!(hook.condition, HookCondition::MentionReceived { .. });

        if is_channel_hook && flags.suppress_channel_hooks() {
            continue;
        }
        if is_mention_hook && flags.suppress_mention_hooks() {
            continue;
        }

        // Check require_flag: if set, the message must contain the specified !flag
        if let Some(ref required) = hook.require_flag
            && !flags.has_custom_flag(required)
        {
            continue;
        }

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
                            // Check if pattern is available (properly deduplicates by claim ID)
                            !is_pattern_held(&pattern_clone, existing_claims, now)
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
                            // Check if pattern is available (properly deduplicates by claim ID)
                            !is_pattern_held(&pattern_clone, existing_claims, now)
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

                // If hook has an explicit --claim pattern, acquire it atomically
                if let (Some(pattern), Some(release)) = (&hook.claim_pattern, &hook.claim_release) {
                    let claim_agent = hook.claim_owner.as_deref().unwrap_or(agent);
                    let pattern_clone = pattern.clone();

                    match release {
                        ClaimRelease::Ttl { secs } => {
                            let ttl = *secs;
                            let c = FileClaim::new(claim_agent, vec![pattern.clone()], ttl);

                            let acquired = append_if(&claims_path(), &c, |existing_claims| {
                                let now = Utc::now();
                                !is_pattern_held(&pattern_clone, existing_claims, now)
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
                                    reason: Some("claim unavailable".to_string()),
                                };
                                let _ = append_record(&hooks_audit_path(), &firing);
                                continue;
                            }

                            (Some(c), Some(ttl), Some(pattern.clone()))
                        }
                        ClaimRelease::OnExit => {
                            let c = FileClaim::new(claim_agent, vec![pattern.clone()], 86400);

                            let acquired = append_if(&claims_path(), &c, |existing_claims| {
                                let now = Utc::now();
                                !is_pattern_held(&pattern_clone, existing_claims, now)
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
                                    reason: Some("claim unavailable".to_string()),
                                };
                                let _ = append_record(&hooks_audit_path(), &firing);
                                continue;
                            }

                            (Some(c), None, Some(pattern.clone()))
                        }
                    }
                } else {
                    // No claim — just fire on mention
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
            let mut command = std::process::Command::new(&hook.command[0]);
            command
                .args(&hook.command[1..])
                .current_dir(&hook.cwd)
                .env("RITE_CHANNEL", channel)
                .env("RITE_MESSAGE_ID", message_id)
                .env("RITE_AGENT", agent)
                .env("RITE_HOOK_ID", &hook.id)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            if let Some(traceparent) = crate::telemetry::current_traceparent() {
                command.env("TRACEPARENT", traceparent);
            }

            match command.spawn() {
                Ok(mut child) => {
                    if is_on_exit {
                        // Block until command exits, then release claim
                        let _ = child.wait();
                        if let Some(c) = &claim {
                            let _ = append_record(&claims_path(), &c.release());
                        }
                    } else {
                        // Reap child in background to prevent zombie processes
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
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
            info!(hook_id = %hook.id, channel, message_id, "hook fired");
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
                claim_pattern: None,
                claim_owner: None,
                priority: 0,
                require_flag: None,
                active: true,
                description: None,
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
                claim_pattern: None,
                claim_owner: None,
                priority: 0,
                require_flag: None,
                active: false, // Deactivated
                description: None,
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

    #[test]
    fn test_priority_sorting() {
        // Create hooks with different priorities
        let hooks = vec![
            Hook {
                id: "hk-high".to_string(),
                channel: "test".to_string(),
                condition: HookCondition::ClaimAvailable {
                    pattern: "p1".to_string(),
                },
                command: vec!["echo".to_string(), "high".to_string()],
                cwd: PathBuf::from("/tmp"),
                cooldown_secs: 30,
                last_fired: None,
                created_at: Utc::now(),
                created_by: None,
                claim_release: Some(ClaimRelease::OnExit),
                claim_pattern: None,
                claim_owner: None,
                priority: 10,
                require_flag: None,
                active: true,
                description: None,
            },
            Hook {
                id: "hk-low".to_string(),
                channel: "test".to_string(),
                condition: HookCondition::ClaimAvailable {
                    pattern: "p2".to_string(),
                },
                command: vec!["echo".to_string(), "low".to_string()],
                cwd: PathBuf::from("/tmp"),
                cooldown_secs: 30,
                last_fired: None,
                created_at: Utc::now(),
                created_by: None,
                claim_release: Some(ClaimRelease::OnExit),
                claim_pattern: None,
                claim_owner: None,
                priority: -5,
                require_flag: None,
                active: true,
                description: None,
            },
            Hook {
                id: "hk-mid".to_string(),
                channel: "test".to_string(),
                condition: HookCondition::ClaimAvailable {
                    pattern: "p3".to_string(),
                },
                command: vec!["echo".to_string(), "mid".to_string()],
                cwd: PathBuf::from("/tmp"),
                cooldown_secs: 30,
                last_fired: None,
                created_at: Utc::now(),
                created_by: None,
                claim_release: Some(ClaimRelease::OnExit),
                claim_pattern: None,
                claim_owner: None,
                priority: 0,
                require_flag: None,
                active: true,
                description: None,
            },
        ];

        let active = build_active_hooks(&hooks);
        let mut hooks_to_process: Vec<&Hook> = active.values().collect();
        hooks_to_process.sort_by_key(|h| h.priority);

        // Verify order: low (-5), mid (0), high (10)
        assert_eq!(hooks_to_process.len(), 3);
        assert_eq!(hooks_to_process[0].id, "hk-low");
        assert_eq!(hooks_to_process[1].id, "hk-mid");
        assert_eq!(hooks_to_process[2].id, "hk-high");
    }
}
