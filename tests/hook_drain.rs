//! A trigger queued behind a spawn lease must not wait for the channel to
//! speak again (bn-38co).
//!
//! The lease batches a trigger that arrives while a spawn is live, and hands
//! it to the next spawn. Nothing schedules that next spawn: it happens only
//! when a *later* message fires the same hook. If the channel goes quiet the
//! trigger waits forever. Observed on #console, where a review approval sat
//! undelivered for two days behind a lease that had lapsed twenty minutes
//! after it was queued.
//!
//! rite has no daemon and this is not the feature to grow one for, so the
//! drain rides traffic that already exists: any invocation that evaluates
//! hooks re-checks pending queues in every channel. These tests pin that it
//! delivers when the lease is gone, refuses to while it is held, attributes
//! the spawn to the queued channel rather than the sweeping one, and cannot
//! turn a failing spawn into a retry storm.
//!
//! Every test runs against its own `RITE_DATA_DIR` and spawns nothing but
//! `sh`.

mod common;

use common::TestProject;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Lease TTL short enough that a test can outlive it.
const SHORT_LEASE_SECS: &str = "1";

fn hooks_file(project: &TestProject) -> PathBuf {
    project.data_path().join("hooks.jsonl")
}

fn queue_file(project: &TestProject) -> PathBuf {
    project.data_path().join("hook_queue.jsonl")
}

fn json_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .collect()
}

/// Queue entries rite would still act on: latest record per entry id wins,
/// then keep the ones never delivered. Reading raw `delivered:false` records
/// instead of this is how a delivered queue reads as a stranded one.
fn pending(project: &TestProject) -> Vec<Value> {
    let mut latest: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for entry in json_lines(&queue_file(project)) {
        latest.insert(entry["id"].as_str().expect("entry id").to_string(), entry);
    }
    latest
        .into_values()
        .filter(|e| !e["delivered"].as_bool().unwrap_or(false))
        .collect()
}

/// One line per spawn: the channel, anchor, and batch the command actually saw.
struct SpawnLog(PathBuf);

impl SpawnLog {
    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.0)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Spawns are detached, so their output lands after `rite` has exited.
    /// Waits for at least `n` of them, then returns everything logged.
    fn wait_for(&self, n: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.lines().len() >= n {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.lines()
    }

    /// Give a spawn that must NOT happen time to happen anyway.
    fn settle(&self) -> Vec<String> {
        std::thread::sleep(Duration::from_millis(400));
        self.lines()
    }

    fn channel(line: &str) -> &str {
        line.split('|').next().unwrap_or_default()
    }

    fn anchor(line: &str) -> &str {
        line.split('|').nth(1).unwrap_or_default()
    }

    fn batch(line: &str) -> Vec<&str> {
        line.split('|')
            .nth(2)
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// A mention hook whose command records the environment each spawn receives.
///
/// `--claim-owner` matters: without it the command agent is whoever sent the
/// message, so every trigger looks like the spawned agent's own chatter and is
/// suppressed rather than queued. Production hooks set it for the same reason.
fn add_hook(project: &TestProject, extra: &[&str]) -> (String, SpawnLog) {
    let cwd = project.work_dir().to_string_lossy().to_string();
    let log = project.work_dir().join("spawns.log");
    let script = format!(
        r#"printf '%s|%s|%s\n' "$RITE_CHANNEL" "$RITE_MESSAGE_ID" "$RITE_BATCH_MESSAGE_IDS" >> {}"#,
        log.display()
    );

    let mut args = vec![
        "hooks",
        "add",
        "--channel",
        "rite",
        "--mention",
        "worker",
        "--claim-owner",
        "responder",
        "--cwd",
        &cwd,
    ];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--", "sh", "-c", &script]);

    project
        .run_rite_with_env(&args, Some("ops"))
        .assert_success();

    let id = json_lines(&hooks_file(project))
        .pop()
        .expect("hooks add must write a record")["id"]
        .as_str()
        .expect("hook id")
        .to_string();

    (id, SpawnLog(log))
}

fn leased_hook(project: &TestProject) -> (String, SpawnLog) {
    add_hook(project, &["--lease", "--lease-ttl", SHORT_LEASE_SECS])
}

fn send(project: &TestProject, channel: &str, body: &str, agent: &str) -> String {
    let out = project.run_rite_with_env(&["send", channel, body, "--format", "json"], Some(agent));
    out.assert_success();
    let parsed: Value = serde_json::from_str(&out.stdout_str()).expect("valid json");
    parsed["id"].as_str().expect("message id").to_string()
}

/// Make the hook look last-fired long enough ago to be sweep-eligible.
///
/// The sweep floor is measured against `last_fired`, so a test that has just
/// fired the hook is inside it. hooks.jsonl is append-only with
/// latest-record-wins, so an amended copy is a legitimate way to age it —
/// the same way rite itself records every change.
fn age_last_fired(project: &TestProject, hook_id: &str, secs: i64) {
    let mut record = json_lines(&hooks_file(project))
        .into_iter()
        .filter(|r| r["id"] == hook_id)
        .next_back()
        .expect("hook record");
    let aged = chrono::Utc::now() - chrono::Duration::seconds(secs);
    record["last_fired"] = Value::String(aged.to_rfc3339());

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(hooks_file(project))
        .expect("open hooks.jsonl");
    use std::io::Write;
    writeln!(file, "{}", serde_json::to_string(&record).unwrap()).expect("append hook record");
}

/// Wait out a lease whose TTL is [`SHORT_LEASE_SECS`].
fn wait_for_lease_to_lapse() {
    std::thread::sleep(Duration::from_millis(1300));
}

/// The headline: a stranded trigger is delivered by traffic in another
/// channel, and the spawn it produces belongs to the channel the trigger came
/// from — not the one that happened to be busy.
#[test]
fn test_stranded_trigger_is_delivered_by_unrelated_traffic() {
    let project = TestProject::with_name("drain-strand");
    let (hook_id, log) = leased_hook(&project);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);

    let stranded = send(&project, "rite", "@worker second", "ops");
    assert_eq!(
        pending(&project).len(),
        1,
        "the second trigger must be queued behind the live lease"
    );

    wait_for_lease_to_lapse();
    age_last_fired(&project, &hook_id, 120);

    // Nothing about this send concerns the hook: different channel, no
    // mention. It is only a rite invocation that evaluates hooks.
    send(&project, "other", "unrelated chatter", "ops");

    let lines = log.wait_for(2);
    assert_eq!(lines.len(), 2, "the stranded trigger must spawn: {lines:?}");
    assert_eq!(
        SpawnLog::channel(&lines[1]),
        "rite",
        "the spawn belongs to the queued channel, not the sweeping one: {lines:?}"
    );
    assert_eq!(
        SpawnLog::anchor(&lines[1]),
        stranded,
        "the spawn must be anchored to the queued message: {lines:?}"
    );
    assert!(
        pending(&project).is_empty(),
        "a delivered trigger must not stay pending: {:?}",
        pending(&project)
    );
}

/// The lease still means what it meant. A sweep that ran while a spawn was
/// live would be exactly the double-spawn the lease exists to prevent.
#[test]
fn test_sweep_does_not_spawn_while_lease_is_held() {
    let project = TestProject::with_name("drain-lease-held");
    // Long enough that the lease is unambiguously still held.
    let (hook_id, log) = add_hook(&project, &["--lease", "--lease-ttl", "600"]);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    send(&project, "rite", "@worker second", "ops");

    // Age the hook so the sweep is genuinely attempted: the only thing left
    // to stop it is the lease.
    age_last_fired(&project, &hook_id, 120);
    send(&project, "other", "unrelated chatter", "ops");

    assert_eq!(
        log.settle().len(),
        1,
        "a held lease must block the sweep the same way it blocks a message"
    );
    assert_eq!(
        pending(&project).len(),
        1,
        "the trigger stays queued for the spawn that is still running"
    );
}

/// A spawn that succeeds empties its queue, so it cannot repeat. A spawn that
/// *fails* correctly leaves its batch queued — and that is the storm the floor
/// exists to bound: without it, every rite command run by any agent anywhere
/// on the machine would retry a hook whose command is missing.
///
/// A broken command is not hypothetical. Eight of forty-two live hooks had a
/// missing `cwd` or command when `doctor` first learned to look (bn-3q9e).
#[test]
fn test_a_failing_sweep_is_not_retried_within_the_floor() {
    let project = TestProject::with_name("drain-floor");
    let (hook_id, log) = leased_hook(&project);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    send(&project, "rite", "@worker second", "ops");
    wait_for_lease_to_lapse();

    // The command goes missing after the trigger was queued — a moved
    // project, a renamed binary. The queue survives the edit.
    project
        .run_rite_with_env(
            &["hooks", "set", &hook_id, "--", "/nonexistent/responder"],
            Some("ops"),
        )
        .assert_success();

    age_last_fired(&project, &hook_id, 120);
    send(&project, "other", "sweep one", "ops");
    assert_eq!(
        failed_sweeps(&project),
        1,
        "the sweep must attempt the spawn once"
    );
    assert_eq!(
        pending(&project).len(),
        1,
        "a spawn that never started must not count as delivery"
    );

    // Still stranded, still eligible but for the floor. Three more chances.
    for body in ["sweep two", "sweep three", "sweep four"] {
        send(&project, "other", body, "ops");
    }
    assert_eq!(
        failed_sweeps(&project),
        1,
        "a failing spawn must not be retried by every message on the machine"
    );

    // The floor is a delay, not a give-up: once it passes, the retry happens.
    age_last_fired(&project, &hook_id, 120);
    send(&project, "other", "sweep five", "ops");
    assert_eq!(
        failed_sweeps(&project),
        2,
        "past the floor the sweep tries again"
    );
    assert_eq!(log.settle().len(), 1, "the broken command never ran");
}

/// Audit records for a sweep whose spawn did not start.
fn failed_sweeps(project: &TestProject) -> usize {
    json_lines(&project.data_path().join("hooks_audit.jsonl"))
        .iter()
        .filter(|r| r["reason"] == "spawn failed" && r["executed"] == false)
        .count()
}

/// `hooks drain --dry-run` is how you see what is stranded without changing
/// anything — including without taking the lease.
#[test]
fn test_drain_dry_run_reports_without_delivering() {
    let project = TestProject::with_name("drain-dry-run");
    let (_hook_id, log) = leased_hook(&project);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    send(&project, "rite", "@worker second", "ops");
    wait_for_lease_to_lapse();

    let out = project.run_rite_with_env(
        &["hooks", "drain", "--dry-run", "--format", "json"],
        Some("ops"),
    );
    out.assert_success();
    let parsed: Value = serde_json::from_str(&out.stdout_str()).expect("valid json");
    let drained = parsed["drained"].as_array().expect("drained array");

    assert_eq!(drained.len(), 1, "one stranded queue: {drained:?}");
    assert_eq!(drained[0]["outcome"], "would spawn");
    assert_eq!(drained[0]["channel"], "rite");
    assert_eq!(parsed["dry_run"], true);

    assert_eq!(log.settle().len(), 1, "a dry run must not spawn");
    assert_eq!(
        pending(&project).len(),
        1,
        "a dry run must not deliver the trigger"
    );
}

/// The escape hatch: deliver now, without waiting for traffic or for the
/// floor. The sweep needs something to ride; this is what to do when there is
/// nothing.
#[test]
fn test_drain_delivers_on_demand() {
    let project = TestProject::with_name("drain-on-demand");
    let (_hook_id, log) = leased_hook(&project);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    let stranded = send(&project, "rite", "@worker second", "ops");
    wait_for_lease_to_lapse();

    // No age_last_fired: --force is exactly what makes this usable by hand.
    let out = project.run_rite_with_env(&["hooks", "drain", "--format", "json"], Some("ops"));
    out.assert_success();
    let parsed: Value = serde_json::from_str(&out.stdout_str()).expect("valid json");
    assert_eq!(parsed["drained"][0]["outcome"], "spawned");

    let lines = log.wait_for(2);
    assert_eq!(lines.len(), 2, "drain must spawn: {lines:?}");
    assert_eq!(SpawnLog::anchor(&lines[1]), stranded);
    assert!(pending(&project).is_empty());
}

/// A batch reads the same whether a message or a sweep produced it:
/// chronological, anchored on the newest.
#[test]
fn test_swept_batch_is_chronological_with_the_newest_last() {
    let project = TestProject::with_name("drain-batch-order");
    let (hook_id, log) = leased_hook(&project);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    let second = send(&project, "rite", "@worker second", "ops");
    let third = send(&project, "rite", "@worker third", "ops");

    wait_for_lease_to_lapse();
    age_last_fired(&project, &hook_id, 120);
    send(&project, "other", "unrelated chatter", "ops");

    let lines = log.wait_for(2);
    assert_eq!(lines.len(), 2, "one spawn for both queued triggers");
    assert_eq!(
        SpawnLog::batch(&lines[1]),
        vec![second.as_str(), third.as_str()],
        "oldest first, anchor last"
    );
    assert_eq!(
        SpawnLog::anchor(&lines[1]),
        third,
        "the anchor is the newest queued trigger"
    );
}

/// Removing a hook orphans its queue permanently: every delivery path matches
/// on the hook id, and that id is gone.
#[test]
fn test_remove_retires_the_pending_queue() {
    let project = TestProject::with_name("drain-remove");
    let (hook_id, log) = add_hook(&project, &["--lease", "--lease-ttl", "600"]);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    send(&project, "rite", "@worker second", "ops");
    assert_eq!(pending(&project).len(), 1, "precondition: one queued");

    let out = project.run_rite_with_env(
        &["hooks", "remove", &hook_id, "--format", "json"],
        Some("ops"),
    );
    out.assert_success();
    let parsed: Value = serde_json::from_str(&out.stdout_str()).expect("valid json");
    assert_eq!(parsed["retired_queue"], 1);

    assert!(
        pending(&project).is_empty(),
        "a queue nothing can deliver must not be left pending: {:?}",
        pending(&project)
    );
}

/// The lease is taken before the condition is evaluated, so without an
/// explicit check a mention hook queues every message in its channel for as
/// long as a spawn is live — and the sweep, which cannot re-derive the
/// mention, would then spawn for messages that were never its work.
#[test]
fn test_unaddressed_message_is_not_queued() {
    let project = TestProject::with_name("drain-unaddressed");
    let (_hook_id, log) = add_hook(&project, &["--lease", "--lease-ttl", "600"]);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);

    // Same channel, lease held, but this one is addressed to nobody.
    send(&project, "rite", "just talking amongst ourselves", "ops");

    assert!(
        pending(&project).is_empty(),
        "a message that does not mention the hook's agent is not its work: {:?}",
        pending(&project)
    );
}

/// The hook's own claim still gates a swept spawn. This is the production
/// shape: a responder hook gated on `agent://<name>` must not start a second
/// agent merely because an old trigger is waiting. The lease says "no spawn of
/// mine is live"; the claim says "that agent is free". Both must hold.
#[test]
fn test_sweep_respects_the_hooks_own_claim() {
    let project = TestProject::with_name("drain-claim-held");
    let (hook_id, log) = add_hook(
        &project,
        &[
            "--claim",
            "agent://responder",
            "--ttl",
            "600",
            "--lease",
            "--lease-ttl",
            SHORT_LEASE_SECS,
        ],
    );

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    send(&project, "rite", "@worker second", "ops");
    wait_for_lease_to_lapse();
    age_last_fired(&project, &hook_id, 120);

    // The first spawn took `agent://responder` for 600s and has not given it
    // back, so the lease has lapsed while the agent is still busy. Only the
    // claim stands between the sweep and a second agent.
    let out = project.run_rite_with_env(&["hooks", "drain", "--format", "json"], Some("ops"));
    out.assert_success();
    let parsed: Value = serde_json::from_str(&out.stdout_str()).expect("valid json");
    assert_eq!(
        parsed["drained"][0]["outcome"], "condition not met",
        "a held claim must stop a swept spawn: {parsed}"
    );
    assert_eq!(log.settle().len(), 1, "no second agent");
    assert_eq!(
        pending(&project).len(),
        1,
        "the trigger stays queued for when the agent is free"
    );
}

/// A hook with no queue costs a sweep nothing, and a channel with no leased
/// hook at all never reads the queue file.
#[test]
fn test_drain_reports_nothing_when_nothing_is_stranded() {
    let project = TestProject::with_name("drain-empty");
    let (_hook_id, log) = leased_hook(&project);

    send(&project, "rite", "@worker first", "ops");
    log.wait_for(1);
    wait_for_lease_to_lapse();

    let out = project.run_rite_with_env(&["hooks", "drain", "--format", "json"], Some("ops"));
    out.assert_success();
    let parsed: Value = serde_json::from_str(&out.stdout_str()).expect("valid json");
    assert_eq!(
        parsed["drained"].as_array().expect("array").len(),
        0,
        "nothing was queued, so nothing is stranded"
    );
    assert_eq!(log.settle().len(), 1, "drain must not re-spawn a hook");
}
