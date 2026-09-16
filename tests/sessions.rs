//! `rite sessions`: live harness attachments and the `agent://` occupancy
//! claim they stake. Acceptance for bn-3c4d (notes/agent-sessions.md,
//! phase 2): two named agents in one directory attach independently; ending
//! one releases only its own claim; a late detach for a session that was
//! never attached is a no-op.

mod common;

use common::TestProject;
use std::path::Path;
use std::time::{Duration, Instant};

fn json(out: &common::RiteOutput) -> serde_json::Value {
    let s = out.stdout_str();
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// Claims that are active *and* unexpired. The shared `active_claims` helper
/// only looks at the active flag, which stays true after the TTL lapses.
fn live_claims(project: &TestProject) -> Vec<serde_json::Value> {
    let now = chrono::Utc::now();
    project
        .active_claims()
        .into_iter()
        .filter(|c| {
            c["expires_at"]
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t > now)
        })
        .collect()
}

fn held_patterns(project: &TestProject) -> Vec<String> {
    live_claims(project)
        .iter()
        .flat_map(|c| {
            c["patterns"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.as_str().map(String::from))
        })
        .collect()
}

#[test]
fn attach_stakes_the_occupancy_claim_and_detach_releases_it() {
    let mut project = TestProject::with_name("sessions-basic");
    let agent = project.agent("codex-a");

    let out = agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid-a",
        "--format",
        "json",
    ]);
    out.assert_success();
    let attached = json(&out);
    assert_eq!(attached["agent"], "codex-a");
    assert_eq!(attached["kind"], "push");
    assert_eq!(attached["claim"]["pattern"], "agent://codex-a");
    assert!(held_patterns(&project).contains(&"agent://codex-a".to_string()));

    let listed = json(&agent.run(&["sessions", "list", "--format", "json"]));
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["occupancy"], "held");
    assert_eq!(listed["sessions"][0]["session"], "sid-a");

    // Detach needs no identity: a SessionEnd hook only knows the session id.
    let out = project.run_rite_with_env(
        &[
            "sessions",
            "detach",
            "--session",
            "sid-a",
            "--format",
            "json",
        ],
        None,
    );
    out.assert_success();
    let detached = json(&out);
    assert_eq!(detached["detached"], attached["attachment_id"]);
    assert_eq!(detached["released_claim"], attached["claim"]["id"]);
    assert!(!held_patterns(&project).contains(&"agent://codex-a".to_string()));
    assert!(
        json(&agent.run(&["sessions", "list", "--format", "json"]))["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn late_detach_for_an_unknown_session_is_a_noop() {
    let mut project = TestProject::with_name("sessions-late");
    let agent = project.agent("codex-a");
    agent
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "real",
            "--format",
            "json",
        ])
        .assert_success();

    // The launch-time placeholder thread ends after the real thread started.
    let out = project.run_rite_with_env(
        &[
            "sessions",
            "detach",
            "--session",
            "placeholder",
            "--format",
            "json",
        ],
        None,
    );
    out.assert_success();
    let noop = json(&out);
    assert!(noop["detached"].is_null());
    assert!(noop["released_claim"].is_null());

    // The real attachment and its claim are untouched.
    assert!(held_patterns(&project).contains(&"agent://codex-a".to_string()));
    let listed = json(&agent.run(&["sessions", "list", "--format", "json"]));
    assert_eq!(listed["sessions"][0]["session"], "real");

    // Detaching twice is also a no-op.
    project
        .run_rite_with_env(&["sessions", "detach", "--session", "real"], None)
        .assert_success();
    let again = json(&project.run_rite_with_env(
        &[
            "sessions",
            "detach",
            "--session",
            "real",
            "--format",
            "json",
        ],
        None,
    ));
    assert!(again["detached"].is_null());
}

#[test]
fn two_agents_in_one_directory_attach_and_detach_independently() {
    let mut project = TestProject::with_name("sessions-pair");
    let a = project.agent("codex-a");
    let b = project.agent("codex-b");

    a.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid-a",
    ])
    .assert_success();
    b.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid-b",
    ])
    .assert_success();

    let held = held_patterns(&project);
    assert!(held.contains(&"agent://codex-a".to_string()));
    assert!(held.contains(&"agent://codex-b".to_string()));

    // End one while the other survives.
    project
        .run_rite_with_env(&["sessions", "detach", "--session", "sid-a"], None)
        .assert_success();

    let held = held_patterns(&project);
    assert!(
        !held.contains(&"agent://codex-a".to_string()),
        "a's claim released"
    );
    assert!(
        held.contains(&"agent://codex-b".to_string()),
        "b's claim untouched"
    );

    let listed = json(&b.run(&["sessions", "list", "--all", "--format", "json"]));
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let by_agent = |name: &str| {
        sessions
            .iter()
            .find(|s| s["agent"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(by_agent("codex-a")["attached"], false);
    assert_eq!(by_agent("codex-b")["attached"], true);
    assert_eq!(by_agent("codex-b")["occupancy"], "held");
}

#[test]
fn one_live_attachment_per_agent_unless_replaced() {
    let mut project = TestProject::with_name("sessions-replace");
    let agent = project.agent("codex-a");
    let first = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--format",
        "json",
    ]));
    let first_id = first["attachment_id"].as_str().unwrap().to_string();

    // A second attach without --replace is refused and names the live one.
    let refused = agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "new",
    ]);
    refused.assert_failure();
    assert!(refused.stderr_str().contains(&first_id));

    // The same session id cannot be attached twice either.
    project
        .agent("codex-b")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "old",
        ])
        .assert_failure();

    // --replace retires the old attachment and its claim, then attaches.
    let replaced = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "new",
        "--replace",
        &first_id,
        "--format",
        "json",
    ]));
    assert_eq!(replaced["replaced"], first_id);
    let listed = json(&agent.run(&["sessions", "list", "--format", "json"]));
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["session"], "new");
    assert_eq!(listed["sessions"][0]["occupancy"], "held");

    // Exactly one live agent://codex-a claim.
    let held: Vec<_> = held_patterns(&project)
        .into_iter()
        .filter(|p| p == "agent://codex-a")
        .collect();
    assert_eq!(held.len(), 1);
}

#[test]
fn attach_refuses_an_ownerless_claim_of_its_own_agent() {
    // Round 4: ownerless occupancy is exactly what a responder hook holds
    // while it runs, and it cannot give detach or renew anything to own.
    let mut project = TestProject::with_name("sessions-ownerless");
    let agent = project.agent("codex-a");
    agent.claim(&["agent://codex-a"]).assert_success();

    let out = agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
    ]);
    out.assert_failure();
    assert!(
        out.stderr_str().contains("without a session owner"),
        "{}",
        out.stderr_str()
    );
    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] == "detached")
    );

    agent.release(&["agent://codex-a"]).assert_success();
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--format",
        "json",
    ]));
    assert_eq!(attached["claim"]["pattern"], "agent://codex-a");
}

#[test]
fn attach_refuses_an_identity_held_by_someone_else() {
    let mut project = TestProject::with_name("sessions-conflict");
    project
        .agent("other")
        .claim(&["agent://codex-a"])
        .assert_success();
    let out = project.agent("codex-a").run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
    ]);
    out.assert_failure();
    assert!(out.stderr_str().contains("held by other"));
}

#[test]
fn renew_extends_the_claim_from_the_bridge() {
    let mut project = TestProject::with_name("sessions-renew");
    let agent = project.agent("codex-a");
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--ttl",
        "60",
        "--format",
        "json",
    ]));
    let id = attached["attachment_id"].as_str().unwrap().to_string();
    let before = attached["claim"]["expires_at"]
        .as_str()
        .unwrap()
        .to_string();

    let renewed = json(&project.run_rite_with_env(
        &[
            "sessions",
            "renew",
            "--attachment",
            &id,
            "--ttl",
            "2h",
            "--format",
            "json",
        ],
        None,
    ));
    let after = renewed["claim"]["expires_at"].as_str().unwrap().to_string();
    assert!(
        after > before,
        "renew must push expiry out: {before} -> {after}"
    );
    assert_eq!(
        renewed["claim"]["id"], attached["claim"]["id"],
        "same claim, extended"
    );
}

fn wait_for_file(path: &Path, want: bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() == want {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The point of the claim: a responder hook gated on `agent://<name>` does
/// not spawn while that agent is attached, and does once it detaches.
#[test]
fn an_attached_agent_suppresses_its_responder_hook() {
    let mut project = TestProject::with_name("sessions-hook");
    let responder = project.agent("responder");
    let marker = project.work_dir().join("spawned.txt");
    let cwd = project.work_dir().display().to_string();
    let cmd = format!("touch {}", marker.display());

    project
        .run_rite_with_env(
            &[
                "hooks",
                "add",
                "--channel",
                "general",
                "--claim",
                "agent://responder",
                "--ttl",
                "600",
                "--cwd",
                &cwd,
                "--",
                "sh",
                "-c",
                &cmd,
            ],
            Some("ops"),
        )
        .assert_success();

    responder
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "sid",
        ])
        .assert_success();
    project
        .agent("someone")
        .send("general", "hello while attached")
        .assert_success();
    assert!(
        !wait_for_file(&marker, true, Duration::from_millis(800)),
        "hook spawned although the agent is attached"
    );

    project
        .run_rite_with_env(&["sessions", "detach", "--session", "sid"], None)
        .assert_success();
    project
        .agent("someone")
        .send("general", "hello after detach")
        .assert_success();
    assert!(
        wait_for_file(&marker, true, Duration::from_secs(10)),
        "hook should spawn once the agent detached"
    );
}

fn parallel_attaches(project: &TestProject, args: Vec<Vec<String>>) -> Vec<bool> {
    // All processes start together on a barrier so the compare-and-append
    // under the file lock is what decides the winner, not launch order.
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(args.len()));
    let data_path = project.data_path().to_path_buf();
    let work_dir = project.work_dir().to_path_buf();
    let handles: Vec<_> = args
        .into_iter()
        .map(|argv| {
            let barrier = barrier.clone();
            let data_path = data_path.clone();
            let work_dir = work_dir.clone();
            std::thread::spawn(move || {
                barrier.wait();
                std::process::Command::new(common::rite_bin())
                    .args(&argv)
                    .env("RITE_DATA_DIR", &data_path)
                    .current_dir(&work_dir)
                    .output()
                    .expect("spawn rite")
                    .status
                    .success()
            })
        })
        .collect();
    handles.into_iter().map(|h| h.join().unwrap()).collect()
}

/// risk:high finding 1: uniqueness must be decided under the append lock.
#[test]
fn concurrent_attaches_for_one_agent_leave_exactly_one_attachment_and_claim() {
    let mut project = TestProject::with_name("sessions-race-agent");
    let agent = project.agent("codex-a");
    let args: Vec<Vec<String>> = (0..8)
        .map(|i| {
            [
                "sessions",
                "attach",
                "--agent",
                "codex-a",
                "--harness",
                "codex",
                "--session",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once(format!("sid-{i}")))
            .collect()
        })
        .collect();
    let wins = parallel_attaches(&project, args)
        .into_iter()
        .filter(|w| *w)
        .count();
    assert_eq!(wins, 1, "exactly one concurrent attach may succeed");

    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    let live = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["attached"] == true)
        .count();
    assert_eq!(live, 1, "one live attachment");
    let claims: Vec<_> = held_patterns(&project)
        .into_iter()
        .filter(|p| p == "agent://codex-a")
        .collect();
    assert_eq!(claims.len(), 1, "one occupancy claim");
    assert_eq!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["attached"] == true)
            .next()
            .unwrap()["occupancy"],
        "held"
    );
}

/// risk:high finding 1, other axis: one session id cannot be bound twice.
#[test]
fn concurrent_attaches_for_one_session_leave_exactly_one_attachment() {
    let mut project = TestProject::with_name("sessions-race-session");
    let args: Vec<Vec<String>> = (0..8)
        .map(|i| {
            ["sessions", "attach", "--agent"]
                .iter()
                .map(|s| s.to_string())
                .chain(std::iter::once(format!("agent-{i}")))
                .chain(
                    ["--harness", "codex", "--session", "shared"]
                        .iter()
                        .map(|s| s.to_string()),
                )
                .collect()
        })
        .collect();
    let wins = parallel_attaches(&project, args)
        .into_iter()
        .filter(|w| *w)
        .count();
    assert_eq!(wins, 1, "exactly one agent may bind a session id");
    let listed = json(
        &project
            .agent("viewer")
            .run(&["sessions", "list", "--all", "--format", "json"]),
    );
    let live: Vec<_> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["attached"] == true)
        .collect();
    assert_eq!(live.len(), 1);
    // The losers wrote nothing at all: no detached leftovers either.
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(held_patterns(&project).len(), 1);
}

/// risk:high finding 2: a lapsed claim is never revived over a newer holder.
#[test]
fn renew_after_expiry_fails_when_someone_else_took_the_identity() {
    let mut project = TestProject::with_name("sessions-renew-expired");
    let agent = project.agent("codex-a");
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--ttl",
        "1",
        "--format",
        "json",
    ]));
    let id = attached["attachment_id"].as_str().unwrap().to_string();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !held_patterns(&project).contains(&"agent://codex-a".to_string()),
        "claim lapsed"
    );

    // Another agent takes the identity in the gap.
    project
        .agent("other")
        .claim(&["agent://codex-a"])
        .assert_success();

    let renewed = project.run_rite_with_env(
        &[
            "sessions",
            "renew",
            "--attachment",
            &id,
            "--ttl",
            "1h",
            "--format",
            "json",
        ],
        None,
    );
    renewed.assert_failure();
    assert!(renewed.stderr_str().contains("held by other"));

    // Exactly one live claim for the pattern, and it is the other agent's.
    let mine: Vec<_> = live_claims(&project)
        .into_iter()
        .filter(|c| {
            c["patterns"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "agent://codex-a")
        })
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["agent"], "other");
}

/// The benign side of finding 2: a lapsed claim nobody took is re-staked
/// under the same claim id, so detach still releases it.
#[test]
fn renew_after_expiry_restakes_when_the_identity_is_free() {
    let mut project = TestProject::with_name("sessions-renew-free");
    let agent = project.agent("codex-a");
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--ttl",
        "1",
        "--format",
        "json",
    ]));
    let id = attached["attachment_id"].as_str().unwrap().to_string();
    std::thread::sleep(Duration::from_millis(1500));

    let renewed = json(&project.run_rite_with_env(
        &[
            "sessions",
            "renew",
            "--attachment",
            &id,
            "--ttl",
            "1h",
            "--format",
            "json",
        ],
        None,
    ));
    assert_eq!(renewed["claim"]["id"], attached["claim"]["id"]);
    assert!(held_patterns(&project).contains(&"agent://codex-a".to_string()));

    let detached = json(&project.run_rite_with_env(
        &["sessions", "detach", "--session", "sid", "--format", "json"],
        None,
    ));
    assert_eq!(detached["released_claim"], attached["claim"]["id"]);
    assert!(!held_patterns(&project).contains(&"agent://codex-a".to_string()));
}

/// --replace hands the claim to the new attachment instead of releasing and
/// restaking, so occupancy never has a gap a responder could spawn into.
#[test]
fn replace_inherits_the_claim_without_a_gap() {
    let mut project = TestProject::with_name("sessions-replace-claim");
    let agent = project.agent("codex-a");
    let first = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--format",
        "json",
    ]));
    let first_id = first["attachment_id"].as_str().unwrap().to_string();
    let replaced = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "new",
        "--replace",
        &first_id,
        "--format",
        "json",
    ]));
    assert_eq!(
        replaced["claim"]["id"], first["claim"]["id"],
        "same claim, carried over"
    );
    // Only one claim record ever went live for the pattern.
    let live: Vec<_> = held_patterns(&project)
        .into_iter()
        .filter(|p| p == "agent://codex-a")
        .collect();
    assert_eq!(live.len(), 1);
}

/// risk:high thread 2: after --replace, the old attachment's detach must not
/// release the claim the successor now owns.
#[test]
fn stale_detach_of_a_replaced_attachment_does_not_release_the_successor_claim() {
    let mut project = TestProject::with_name("sessions-stale-detach");
    let agent = project.agent("codex-a");
    let first = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--format",
        "json",
    ]));
    let first_id = first["attachment_id"].as_str().unwrap().to_string();
    agent
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "new",
            "--replace",
            &first_id,
        ])
        .assert_success();

    // The old bridge's SessionEnd hook fires late, naming the old session
    // and, separately, the old attachment id.
    for args in [
        vec!["sessions", "detach", "--session", "old", "--format", "json"],
        vec![
            "sessions",
            "detach",
            "--attachment",
            &first_id,
            "--format",
            "json",
        ],
    ] {
        let out = json(&project.run_rite_with_env(&args, None));
        assert!(
            out["detached"].is_null(),
            "stale detach must be a no-op: {out}"
        );
        assert!(out["released_claim"].is_null());
    }
    assert!(held_patterns(&project).contains(&"agent://codex-a".to_string()));
    let listed = json(&agent.run(&["sessions", "list", "--format", "json"]));
    assert_eq!(listed["sessions"][0]["session"], "new");
    assert_eq!(listed["sessions"][0]["occupancy"], "held");
}

/// risk:high thread 2: the old bridge keeps renewing after a replace. It
/// must be told it lost the identity, and must not extend or release.
#[test]
fn stale_renew_of_a_replaced_attachment_is_refused() {
    let mut project = TestProject::with_name("sessions-stale-renew");
    let agent = project.agent("codex-a");
    let first = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--ttl",
        "1h",
        "--format",
        "json",
    ]));
    let first_id = first["attachment_id"].as_str().unwrap().to_string();
    let second = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "new",
        "--replace",
        &first_id,
        "--ttl",
        "2h",
        "--format",
        "json",
    ]));
    let expiry_after_replace = second["claim"]["expires_at"].as_str().unwrap().to_string();

    let stale = project.run_rite_with_env(
        &[
            "sessions",
            "renew",
            "--attachment",
            &first_id,
            "--ttl",
            "5h",
            "--format",
            "json",
        ],
        None,
    );
    stale.assert_failure();

    // The successor's claim is untouched: same expiry, still held.
    let live = live_claims(&project);
    let mine: Vec<_> = live
        .iter()
        .filter(|c| {
            c["patterns"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "agent://codex-a")
        })
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["expires_at"], expiry_after_replace);
    assert_eq!(mine[0]["owner"], second["attachment_id"]);
}

fn append_session_line(project: &TestProject, line: &str) {
    let path = project.data_path().join("local/sessions.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{line}").unwrap();
}

/// risk:high thread 1: a crash after the detach record but before the
/// release leaves a claim owned by a retired attachment. The next session
/// command for that agent releases it.
#[test]
fn a_claim_owned_by_a_retired_attachment_is_released_on_the_next_command() {
    let mut project = TestProject::with_name("sessions-ghost-claim");
    let agent = project.agent("codex-a");
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--format",
        "json",
    ]));
    // Simulate the crash: write the detached record by hand, release nothing.
    let line = format!(
        r#"{{"ts":"{}","attachment_id":"{}","agent":"codex-a","harness":"codex","session":"sid","kind":"push","event":"detached","claim_id":"{}"}}"#,
        chrono::Utc::now().to_rfc3339(),
        attached["attachment_id"].as_str().unwrap(),
        attached["claim"]["id"].as_str().unwrap()
    );
    append_session_line(&project, &line);
    assert!(
        held_patterns(&project).contains(&"agent://codex-a".to_string()),
        "ghost claim exists"
    );

    // A new attach for the agent reconciles first, then succeeds.
    let again = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid-2",
        "--format",
        "json",
    ]));
    assert_ne!(
        again["claim"]["id"], attached["claim"]["id"],
        "fresh claim, ghost released"
    );
    let mine: Vec<_> = held_patterns(&project)
        .into_iter()
        .filter(|p| p == "agent://codex-a")
        .collect();
    assert_eq!(mine.len(), 1);
}

/// risk:high thread 1: a crash between the reservation and the commit
/// leaves an `attaching` record. Inside the window it still reserves the
/// agent; past PENDING_TTL it is retired and the agent is free again.
#[test]
fn an_abandoned_reservation_is_retired_after_its_ttl() {
    let mut project = TestProject::with_name("sessions-abandoned");
    let agent = project.agent("codex-a");
    let fresh = format!(
        r#"{{"ts":"{}","attachment_id":"{}","agent":"codex-a","harness":"codex","session":"crashed","kind":"push","event":"attaching","claim_id":"{}"}}"#,
        chrono::Utc::now().to_rfc3339(),
        ulid::Ulid::new(),
        ulid::Ulid::new()
    );
    append_session_line(&project, &fresh);
    // Inside the window: the reservation holds and attach is refused.
    agent
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "sid",
        ])
        .assert_failure();

    let stale = format!(
        r#"{{"ts":"{}","attachment_id":"{}","agent":"codex-b","harness":"codex","session":"crashed-b","kind":"push","event":"attaching","claim_id":"{}"}}"#,
        (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339(),
        ulid::Ulid::new(),
        ulid::Ulid::new()
    );
    append_session_line(&project, &stale);
    // Past the window: reconciled away, attach succeeds, and the leftover
    // shows as detached with the abandonment noted.
    project
        .agent("codex-b")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "sid-b",
        ])
        .assert_success();
    let listed = json(&project.agent("codex-b").run(&[
        "sessions", "list", "--name", "codex-b", "--all", "--format", "json",
    ]));
    let states: Vec<String> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            format!(
                "{}:{}",
                s["session"].as_str().unwrap(),
                s["state"].as_str().unwrap()
            )
        })
        .collect();
    assert!(
        states.contains(&"crashed-b:detached".to_string()),
        "{states:?}"
    );
    assert!(states.contains(&"sid-b:attached".to_string()), "{states:?}");
}

/// Round 3, risk:high: a torn or unreadable record must fail closed. No
/// identity is reused or released on the strength of a log this build
/// cannot fully read; `list` still works and reports it.
#[test]
fn a_damaged_session_log_refuses_state_changes() {
    let mut project = TestProject::with_name("sessions-damaged");
    let agent = project.agent("codex-a");
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--format",
        "json",
    ]));
    let id = attached["attachment_id"].as_str().unwrap().to_string();
    append_session_line(
        &project,
        r#"{"ts":"2026-09-16T00:00:00Z","attachment_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","agent":"codex-a","harness":"codex","session":"sid","kind":"push","event":"detached","claim_id":"01ARZ3NDEKTSV4RRFFQ69G5FA"#,
    );

    let listed = json(&agent.run(&["sessions", "list", "--format", "json"]));
    assert_eq!(listed["unreadable_records"], 1);
    assert_eq!(
        listed["sessions"][0]["attachment_id"], id,
        "the readable state is still shown"
    );

    for args in [
        vec![
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "sid-2",
        ],
        vec![
            "sessions",
            "attach",
            "--agent",
            "codex-b",
            "--harness",
            "codex",
            "--session",
            "sid-b",
        ],
        vec!["sessions", "detach", "--session", "sid"],
        vec!["sessions", "renew", "--attachment", &id],
    ] {
        let out = project.run_rite_with_env(&args, Some("codex-a"));
        out.assert_failure();
        assert!(
            out.stderr_str().contains("unreadable"),
            "{:?}: {}",
            args,
            out.stderr_str()
        );
    }
    // Nothing moved: the attachment is still live and still holds its claim.
    assert!(held_patterns(&project).contains(&"agent://codex-a".to_string()));
    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["state"], "attached");
}

/// Round 3, risk:high: a replace that crashed after the claim changed hands
/// but before the successor committed must give the claim back to the live
/// predecessor when the successor is reconciled away.
#[test]
fn an_abandoned_successor_hands_the_claim_back_to_its_live_predecessor() {
    let mut project = TestProject::with_name("sessions-replace-crash");
    let agent = project.agent("codex-a");
    let old = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--ttl",
        "1h",
        "--format",
        "json",
    ]));
    let old_id = old["attachment_id"].as_str().unwrap().to_string();
    let claim_id = old["claim"]["id"].as_str().unwrap().to_string();
    let successor = ulid::Ulid::new().to_string();

    // The successor's reservation, older than PENDING_TTL, naming the old one.
    append_session_line(
        &project,
        &format!(
            r#"{{"ts":"{}","attachment_id":"{}","agent":"codex-a","harness":"codex","session":"new","kind":"push","event":"attaching","claim_id":"{}","replaces":"{}"}}"#,
            (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339(),
            successor,
            claim_id,
            old_id
        ),
    );
    // The claim already re-tagged to the successor, as occupy() does.
    let claims_path = project.data_path().join("claims.jsonl");
    let mut claim: serde_json::Value = std::fs::read_to_string(&claims_path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|c| c["id"] == claim_id)
        .unwrap();
    claim["event"] = serde_json::json!("extended");
    claim["owner"] = serde_json::json!(successor);
    claim["ts"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&claims_path)
            .unwrap();
        writeln!(f, "{}", claim).unwrap();
    }
    assert_eq!(
        live_claims(&project)[0]["owner"],
        successor,
        "precondition: successor owns it"
    );

    // Any session command for the agent reconciles: successor retired, claim
    // back with the old attachment, still held.
    let renewed = json(&project.run_rite_with_env(
        &[
            "sessions",
            "renew",
            "--attachment",
            &old_id,
            "--ttl",
            "2h",
            "--format",
            "json",
        ],
        None,
    ));
    assert_eq!(renewed["claim"]["id"], claim_id);
    let live = live_claims(&project);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0]["owner"], old_id);
    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    let by_session = |s: &str| {
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["session"] == s)
            .unwrap()
            .clone()
    };
    assert_eq!(by_session("old")["state"], "attached");
    assert_eq!(by_session("new")["state"], "detached");
}

/// Round 3, risk:high: between reserving the agent and staking the claim, a
/// responder hook must already treat the identity as busy.
#[test]
fn a_live_reservation_blocks_responder_admission() {
    let mut project = TestProject::with_name("sessions-pending-admission");
    let marker = project.work_dir().join("spawned.txt");
    let cwd = project.work_dir().display().to_string();
    let cmd = format!("touch {}", marker.display());
    project
        .run_rite_with_env(
            &[
                "hooks",
                "add",
                "--channel",
                "general",
                "--claim",
                "agent://responder",
                "--ttl",
                "600",
                "--cwd",
                &cwd,
                "--",
                "sh",
                "-c",
                &cmd,
            ],
            Some("ops"),
        )
        .assert_success();

    // A fresh reservation with no claim yet.
    append_session_line(
        &project,
        &format!(
            r#"{{"ts":"{}","attachment_id":"{}","agent":"responder","harness":"codex","session":"sid","kind":"push","event":"attaching","claim_id":"{}"}}"#,
            chrono::Utc::now().to_rfc3339(),
            ulid::Ulid::new(),
            ulid::Ulid::new()
        ),
    );
    project
        .agent("someone")
        .send("general", "hello while reserved")
        .assert_success();
    assert!(
        !wait_for_file(&marker, true, Duration::from_millis(800)),
        "hook spawned although the agent is reserved"
    );
}

/// Round 3, risk:high: the generic claim commands must not touch a claim
/// bound to a session attachment. `rite claims release --all` is routine
/// end-of-bone cleanup and must leave live occupancy alone.
#[test]
fn generic_release_and_refresh_skip_the_owned_claim() {
    let mut project = TestProject::with_name("sessions-generic-claims");
    let agent = project.agent("codex-a");
    agent.claim(&["src/**"]).assert_success();
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--ttl",
        "1h",
        "--format",
        "json",
    ]));
    let expiry = attached["claim"]["expires_at"]
        .as_str()
        .unwrap()
        .to_string();

    agent
        .run(&["claims", "refresh", "agent://codex-a", "--ttl", "18000"])
        .assert_success();
    agent.release_all().assert_success();

    let live = live_claims(&project);
    let owned: Vec<_> = live
        .iter()
        .filter(|c| {
            c["patterns"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "agent://codex-a")
        })
        .collect();
    assert_eq!(owned.len(), 1, "release --all left the owned claim");
    assert_eq!(
        owned[0]["expires_at"], expiry,
        "refresh left the owned claim"
    );
    assert!(
        !live.iter().any(|c| c["patterns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p.as_str().unwrap().ends_with("src/**"))),
        "the ordinary claim was released"
    );

    // The session command still can.
    let detached = json(&project.run_rite_with_env(
        &["sessions", "detach", "--session", "sid", "--format", "json"],
        None,
    ));
    assert_eq!(detached["released_claim"], attached["claim"]["id"]);
}

/// Round 3, risk:medium: the synced claim carries no harness session id.
#[test]
fn the_occupancy_claim_does_not_carry_the_session_id() {
    let mut project = TestProject::with_name("sessions-claim-message");
    project
        .agent("codex-a")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "secret-session-id-42",
        ])
        .assert_success();
    let claims = std::fs::read_to_string(project.data_path().join("claims.jsonl")).unwrap();
    assert!(
        !claims.contains("secret-session-id-42"),
        "session id leaked into claims.jsonl"
    );
}

/// Round 4, risk:high: an abandoned successor that owns an already expired
/// claim must not be handed back over a newer holder.
#[test]
fn handback_does_not_revive_an_expired_claim_over_a_new_holder() {
    let mut project = TestProject::with_name("sessions-handback-expired");
    let agent = project.agent("codex-a");
    let old = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--ttl",
        "1",
        "--format",
        "json",
    ]));
    let old_id = old["attachment_id"].as_str().unwrap().to_string();
    let claim_id = old["claim"]["id"].as_str().unwrap().to_string();
    let successor = ulid::Ulid::new().to_string();
    append_session_line(
        &project,
        &format!(
            r#"{{"ts":"{}","attachment_id":"{}","agent":"codex-a","harness":"codex","session":"new","kind":"push","event":"attaching","claim_id":"{}","replaces":"{}"}}"#,
            (chrono::Utc::now() - chrono::Duration::seconds(120)).to_rfc3339(),
            successor,
            claim_id,
            old_id
        ),
    );
    let claims_path = project.data_path().join("claims.jsonl");
    let mut claim: serde_json::Value = std::fs::read_to_string(&claims_path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|c| c["id"] == claim_id)
        .unwrap();
    claim["event"] = serde_json::json!("extended");
    claim["owner"] = serde_json::json!(successor);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&claims_path)
            .unwrap();
        writeln!(f, "{}", claim).unwrap();
    }
    std::thread::sleep(Duration::from_millis(1500));
    project
        .agent("other")
        .claim(&["agent://codex-a"])
        .assert_success();

    let renewed = project.run_rite_with_env(
        &["sessions", "renew", "--attachment", &old_id, "--ttl", "1h"],
        None,
    );
    renewed.assert_failure();
    let live = live_claims(&project);
    let holders: Vec<_> = live
        .iter()
        .filter(|c| {
            c["patterns"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "agent://codex-a")
        })
        .collect();
    assert_eq!(holders.len(), 1, "{holders:?}");
    assert_eq!(holders[0]["agent"], "other");
}

/// Round 4, risk:high: a claim-gated responder that won the identity holds
/// an ownerless claim; attach must refuse rather than adopt it, so the two
/// can never both run. Both orderings of the two claim writes reduce to
/// this case or to the hook's stake failing on a held pattern.
#[test]
fn attach_refuses_while_a_responder_holds_the_identity() {
    let mut project = TestProject::with_name("sessions-hook-holds");
    let cwd = project.work_dir().display().to_string();
    project
        .run_rite_with_env(
            &[
                "hooks",
                "add",
                "--channel",
                "general",
                "--claim",
                "agent://responder",
                "--claim-owner",
                "responder",
                "--ttl",
                "600",
                "--cwd",
                &cwd,
                "--",
                "sh",
                "-c",
                "true",
            ],
            Some("ops"),
        )
        .assert_success();
    project
        .agent("someone")
        .send("general", "wake the responder")
        .assert_success();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !held_patterns(&project).contains(&"agent://responder".to_string()) {
        assert!(Instant::now() < deadline, "hook never staked its claim");
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = project.agent("responder").run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
    ]);
    out.assert_failure();
    assert!(
        out.stderr_str().contains("without a session owner"),
        "{}",
        out.stderr_str()
    );
}

/// Round 5, risk:high: a launcher reserves the identity before the harness
/// exists, so there is no gap for a responder to start in; the session id is
/// bound once the harness reports it.
#[test]
fn reserve_holds_the_identity_before_the_harness_exists_and_bind_completes_it() {
    let mut project = TestProject::with_name("sessions-reserve");
    let agent = project.agent("responder");
    let marker = project.work_dir().join("spawned.txt");
    let cwd = project.work_dir().display().to_string();
    let cmd = format!("touch {}", marker.display());
    project
        .run_rite_with_env(
            &[
                "hooks",
                "add",
                "--channel",
                "general",
                "--claim",
                "agent://responder",
                "--claim-owner",
                "responder",
                "--ttl",
                "600",
                "--cwd",
                &cwd,
                "--",
                "sh",
                "-c",
                &cmd,
            ],
            Some("ops"),
        )
        .assert_success();

    let reserved = json(&agent.run(&[
        "sessions",
        "reserve",
        "--harness",
        "codex",
        "--window",
        "5m",
        "--format",
        "json",
    ]));
    let id = reserved["attachment_id"].as_str().unwrap().to_string();
    assert_eq!(reserved["claim"]["pattern"], "agent://responder");
    assert!(held_patterns(&project).contains(&"agent://responder".to_string()));

    // A second reservation is refused; the hook cannot spawn.
    agent
        .run(&["sessions", "reserve", "--harness", "codex"])
        .assert_failure();
    project
        .agent("someone")
        .send("general", "wake the responder")
        .assert_success();
    assert!(
        !wait_for_file(&marker, true, Duration::from_millis(800)),
        "hook spawned over a reservation"
    );

    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    assert_eq!(listed["sessions"][0]["state"], "attaching");
    assert_eq!(listed["sessions"][0]["session"], "(reserved)");

    // The harness starts and reports its id; bind it.
    let bound = json(&agent.run(&[
        "sessions",
        "attach",
        "--attachment",
        &id,
        "--session",
        "sid-real",
        "--format",
        "json",
    ]));
    assert_eq!(bound["attachment_id"], id);
    assert_eq!(
        bound["claim"]["id"], reserved["claim"]["id"],
        "same claim, no gap"
    );
    let listed = json(&agent.run(&["sessions", "list", "--format", "json"]));
    assert_eq!(listed["sessions"][0]["session"], "sid-real");
    assert_eq!(listed["sessions"][0]["state"], "attached");
    assert_eq!(listed["sessions"][0]["occupancy"], "held");

    // Binding again, or binding a different reservation id, is refused.
    agent
        .run(&[
            "sessions",
            "attach",
            "--attachment",
            &id,
            "--session",
            "sid-real",
        ])
        .assert_failure();

    // Detach by the bound session id releases the reservation's claim.
    let detached = json(&project.run_rite_with_env(
        &[
            "sessions",
            "detach",
            "--session",
            "sid-real",
            "--format",
            "json",
        ],
        None,
    ));
    assert_eq!(detached["released_claim"], reserved["claim"]["id"]);
}

/// Round 5, risk:high: every claim transition enters the sync auto-commit
/// path, so `sync push` carries occupancy to other hosts like any claim.
#[test]
fn session_claim_transitions_are_auto_committed() {
    let mut project = TestProject::with_name("sessions-autocommit");
    let agent = project.agent("codex-a");
    let data_path = project.data_path().to_path_buf();
    agent.run(&["sync", "init"]).assert_success();
    std::process::Command::new("git")
        .current_dir(&data_path)
        .args(["config", "commit.gpgsign", "false"])
        .output()
        .unwrap();
    let commits = |msg: &str| -> usize {
        let out = std::process::Command::new("git")
            .current_dir(&data_path)
            .args(["log", "--oneline", "--", "claims.jsonl"])
            .output()
            .unwrap();
        let n = String::from_utf8_lossy(&out.stdout).lines().count();
        eprintln!("{msg}: {n} claim commits");
        n
    };
    let before = commits("start");
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--format",
        "json",
    ]));
    let after_attach = commits("attach");
    assert!(after_attach > before, "attach must commit the claim");
    let id = attached["attachment_id"].as_str().unwrap().to_string();
    project
        .run_rite_with_env(
            &["sessions", "renew", "--attachment", &id, "--ttl", "2h"],
            None,
        )
        .assert_success();
    let after_renew = commits("renew");
    assert!(
        after_renew > after_attach,
        "renew must commit the extension"
    );
    project
        .run_rite_with_env(&["sessions", "detach", "--session", "sid"], None)
        .assert_success();
    assert!(
        commits("detach") > after_renew,
        "detach must commit the release"
    );
    let status = std::process::Command::new("git")
        .current_dir(&data_path)
        .args(["status", "--porcelain", "--", "claims.jsonl"])
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "claims.jsonl left dirty: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

/// Round 5, risk:high: a replacement that was reconciled away *before* the
/// claim changed hands, and whose paused attach then moved the claim, must
/// still give the claim back to the live predecessor rather than release it.
#[test]
fn a_retired_successor_that_ends_up_owning_the_claim_hands_it_back() {
    let mut project = TestProject::with_name("sessions-retired-successor");
    let agent = project.agent("codex-a");
    let old = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "old",
        "--ttl",
        "1h",
        "--format",
        "json",
    ]));
    let old_id = old["attachment_id"].as_str().unwrap().to_string();
    let claim_id = old["claim"]["id"].as_str().unwrap().to_string();
    let successor = ulid::Ulid::new().to_string();
    let t = chrono::Utc::now().to_rfc3339();
    // The successor's reservation, then its retirement by reconciliation.
    append_session_line(
        &project,
        &format!(
            r#"{{"ts":"{t}","attachment_id":"{successor}","agent":"codex-a","harness":"codex","session":"new","kind":"push","event":"attaching","claim_id":"{claim_id}","replaces":"{old_id}"}}"#
        ),
    );
    append_session_line(
        &project,
        &format!(
            r#"{{"ts":"{t}","attachment_id":"{successor}","agent":"codex-a","harness":"codex","session":"new","kind":"push","event":"detached","claim_id":"{claim_id}","replaces":"{old_id}","note":"abandoned attach"}}"#
        ),
    );
    // Then the paused attach's occupy moved the claim to the retired successor.
    let claims_path = project.data_path().join("claims.jsonl");
    let mut claim: serde_json::Value = std::fs::read_to_string(&claims_path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|c| c["id"] == claim_id)
        .unwrap();
    claim["event"] = serde_json::json!("extended");
    claim["owner"] = serde_json::json!(successor);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&claims_path)
            .unwrap();
        writeln!(f, "{}", claim).unwrap();
    }

    // The predecessor's next renew reconciles: the claim comes back, not away.
    let renewed = json(&project.run_rite_with_env(
        &[
            "sessions",
            "renew",
            "--attachment",
            &old_id,
            "--ttl",
            "2h",
            "--format",
            "json",
        ],
        None,
    ));
    assert_eq!(renewed["claim"]["id"], claim_id);
    let live = live_claims(&project);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0]["owner"], old_id);
}

/// Round 6, risk:high: binding a reservation must verify its claim is still
/// live and owned by it. Here the claim was released underneath the
/// reservation and another agent took the identity.
#[test]
fn bind_fails_when_the_reservation_lost_its_claim() {
    let mut project = TestProject::with_name("sessions-bind-lost-claim");
    let agent = project.agent("codex-a");
    let reserved = json(&agent.run(&[
        "sessions",
        "reserve",
        "--harness",
        "codex",
        "--ttl",
        "10m",
        "--window",
        "5m",
        "--format",
        "json",
    ]));
    let id = reserved["attachment_id"].as_str().unwrap().to_string();
    let claim_id = reserved["claim"]["id"].as_str().unwrap().to_string();

    // Simulate the claim going away: an explicit release record, then
    // another holder.
    let claims_path = project.data_path().join("claims.jsonl");
    let mut claim: serde_json::Value = std::fs::read_to_string(&claims_path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|c| c["id"] == claim_id)
        .unwrap();
    claim["event"] = serde_json::json!("released");
    claim["active"] = serde_json::json!(false);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&claims_path)
            .unwrap();
        writeln!(f, "{}", claim).unwrap();
    }
    project
        .agent("other")
        .claim(&["agent://codex-a"])
        .assert_success();

    let bound = agent.run(&[
        "sessions",
        "attach",
        "--attachment",
        &id,
        "--session",
        "sid",
    ]);
    bound.assert_failure();
    assert!(
        bound.stderr_str().contains("no longer holds"),
        "{}",
        bound.stderr_str()
    );
    // No attached record exists, and the other holder is untouched.
    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] == "detached")
    );
    let holders = live_claims(&project);
    assert_eq!(holders.len(), 1);
    assert_eq!(holders[0]["agent"], "other");

    // And a reservation whose claim would not outlive its window is refused
    // up front, including when the two are equal.
    for (ttl, window) in [("1m", "5m"), ("5m", "5m")] {
        agent
            .run(&[
                "sessions",
                "reserve",
                "--harness",
                "codex",
                "--ttl",
                ttl,
                "--window",
                window,
            ])
            .assert_failure();
    }
}

/// Round 7, risk:high: bind renews under the claims lock, so a claim that
/// expired between reservation and bind is caught there and the reservation
/// is retired, while a live one comes out of bind with a fresh TTL and is
/// still rechecked inside the commit.
#[test]
fn bind_renews_the_claim_and_refuses_an_expired_one() {
    let mut project = TestProject::with_name("sessions-bind-renew");
    let agent = project.agent("codex-a");
    let reserved = json(&agent.run(&[
        "sessions",
        "reserve",
        "--harness",
        "codex",
        "--ttl",
        "10m",
        "--window",
        "5m",
        "--format",
        "json",
    ]));
    let id = reserved["attachment_id"].as_str().unwrap().to_string();
    let before = reserved["claim"]["expires_at"]
        .as_str()
        .unwrap()
        .to_string();
    let bound = json(&agent.run(&[
        "sessions",
        "attach",
        "--attachment",
        &id,
        "--session",
        "sid",
        "--ttl",
        "2h",
        "--format",
        "json",
    ]));
    assert_eq!(bound["claim"]["id"], reserved["claim"]["id"]);
    let after = bound["claim"]["expires_at"].as_str().unwrap().to_string();
    assert!(after > before, "bind must renew: {before} -> {after}");

    // A second reservation whose claim is then forced to expire.
    project
        .run_rite_with_env(&["sessions", "detach", "--session", "sid"], None)
        .assert_success();
    let reserved = json(&agent.run(&[
        "sessions",
        "reserve",
        "--harness",
        "codex",
        "--ttl",
        "10m",
        "--window",
        "5m",
        "--format",
        "json",
    ]));
    let id = reserved["attachment_id"].as_str().unwrap().to_string();
    let claim_id = reserved["claim"]["id"].as_str().unwrap().to_string();
    let claims_path = project.data_path().join("claims.jsonl");
    let mut claim: serde_json::Value = std::fs::read_to_string(&claims_path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|c| c["id"] == claim_id)
        .unwrap();
    claim["event"] = serde_json::json!("extended");
    claim["expires_at"] =
        serde_json::json!((chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339());
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&claims_path)
            .unwrap();
        writeln!(f, "{}", claim).unwrap();
    }
    project
        .agent("other")
        .claim(&["agent://codex-a"])
        .assert_success();
    let bound = agent.run(&[
        "sessions",
        "attach",
        "--attachment",
        &id,
        "--session",
        "sid-2",
    ]);
    bound.assert_failure();
    let listed = json(&agent.run(&["sessions", "list", "--all", "--format", "json"]));
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] == "detached")
    );
    let holders = live_claims(&project);
    assert_eq!(holders.len(), 1);
    assert_eq!(holders[0]["agent"], "other");
}

/// Round 6, risk:high: a lifecycle event this build does not know reserves
/// the identity. After an upgrade writes a new live-state event, an older
/// binary must neither attach nor spawn beside it.
#[test]
fn an_unknown_session_event_fails_closed() {
    let mut project = TestProject::with_name("sessions-unknown-event");
    let agent = project.agent("responder");
    let marker = project.work_dir().join("spawned.txt");
    let cwd = project.work_dir().display().to_string();
    let cmd = format!("touch {}", marker.display());
    project
        .run_rite_with_env(
            &[
                "hooks",
                "add",
                "--channel",
                "general",
                "--claim",
                "agent://responder",
                "--claim-owner",
                "responder",
                "--ttl",
                "600",
                "--cwd",
                &cwd,
                "--",
                "sh",
                "-c",
                &cmd,
            ],
            Some("ops"),
        )
        .assert_success();
    let attached = json(&agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid",
        "--format",
        "json",
    ]));
    let id = attached["attachment_id"].as_str().unwrap().to_string();
    // A newer rite appends an event this build does not understand, and the
    // claim is gone (say the newer build moved occupancy elsewhere).
    append_session_line(
        &project,
        &format!(
            r#"{{"ts":"{}","attachment_id":"{}","agent":"responder","harness":"codex","session":"sid","kind":"push","event":"suspended","claim_id":"{}"}}"#,
            chrono::Utc::now().to_rfc3339(),
            id,
            attached["claim"]["id"].as_str().unwrap()
        ),
    );
    // An older build cannot retire what it does not understand.
    project
        .run_rite_with_env(&["sessions", "detach", "--attachment", &id], None)
        .assert_failure();
    agent.release_all().assert_success();

    // Not free: a second attach is refused and the hook does not spawn.
    let again = agent.run(&[
        "sessions",
        "attach",
        "--harness",
        "codex",
        "--session",
        "sid-2",
    ]);
    again.assert_failure();
    project
        .agent("someone")
        .send("general", "hello")
        .assert_success();
    assert!(
        !wait_for_file(&marker, true, Duration::from_millis(800)),
        "hook spawned over an unknown event"
    );
}
