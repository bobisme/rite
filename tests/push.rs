//! Push-at-send: `rite send` delivers a message into the live push-kind
//! session of every agent it addresses, from the sender's own process,
//! through an adapter this host configures. No bridge process is involved.

mod common;

use common::TestProject;
use std::path::{Path, PathBuf};

fn json(out: &common::RiteOutput) -> serde_json::Value {
    let s = out.stdout_str();
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("not JSON: {e}\n{s}"))
}

/// Register a host adapter named `name` in the sending host's table.
fn write_adapter(project: &TestProject, name: &str, argv: &[&str]) {
    let path = project.data_path().join("local/adapters.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut table: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    table.insert(name.to_string(), serde_json::json!(argv));
    std::fs::write(&path, serde_json::Value::Object(table).to_string()).unwrap();
}

fn attach(
    project: &mut TestProject,
    agent: &str,
    adapter: &str,
    session: &str,
) -> serde_json::Value {
    json(&project.agent(agent).run(&[
        "sessions",
        "attach",
        "--harness",
        "custom",
        "--adapter",
        adapter,
        "--session",
        session,
        "--format",
        "json",
    ]))
}

/// Attach `agent` to a push session whose adapter appends the message id,
/// the reply target, and the envelope to `out`. Returns the attachment id.
fn attach_recorder(project: &mut TestProject, agent: &str, out: &Path) -> String {
    let script = format!(
        "printf '%s\\n%s\\n%s\\n---\\n' \"$RITE_MESSAGE_ID\" \"$1\" \"$2\" >> {}",
        out.display()
    );
    write_adapter(
        project,
        "recorder",
        &["sh", "-c", &script, "_", "{reply_target}", "{rendered}"],
    );
    attach(project, agent, "recorder", &format!("sid-{agent}"))["attachment_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn recorded(out: &Path) -> Vec<String> {
    std::fs::read_to_string(out)
        .unwrap_or_default()
        .split("---\n")
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn a_mention_is_pushed_into_the_live_session_with_the_envelope() {
    let mut project = TestProject::with_name("push-mention");
    let out: PathBuf = project.work_dir().join("pushed.txt");
    attach_recorder(&mut project, "codex-a", &out);

    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a please look at this",
        "--format",
        "json",
    ]));
    let deliveries = sent["deliveries"].as_array().expect("deliveries reported");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0]["agent"], "codex-a");
    assert_eq!(deliveries[0]["ok"], true);

    let got = recorded(&out);
    assert_eq!(got.len(), 1, "{got:?}");
    let lines: Vec<&str> = got[0].lines().collect();
    assert_eq!(lines[0], sent["id"].as_str().unwrap(), "RITE_MESSAGE_ID");
    assert_eq!(lines[1], "general", "reply target is the channel");
    assert!(
        lines[2].starts_with("[rite] channel=general from=someone id="),
        "{}",
        lines[2]
    );
    assert!(lines[2].contains("reply_target=general route=mention"));
    assert_eq!(
        lines[3], "@codex-a please look at this",
        "body follows the envelope"
    );
}

#[test]
fn unaddressed_messages_and_the_sessions_own_messages_are_not_pushed() {
    let mut project = TestProject::with_name("push-unaddressed");
    let out = project.work_dir().join("pushed.txt");
    attach_recorder(&mut project, "codex-a", &out);

    project
        .agent("someone")
        .send("general", "nothing for anyone")
        .assert_success();
    project
        .agent("codex-a")
        .send("general", "@codex-a talking to myself")
        .assert_success();
    let sent = json(
        &project
            .agent("someone")
            .run(&["send", "general", "plain", "--format", "json"]),
    );
    assert!(
        sent.get("deliveries").is_none(),
        "no deliveries field when nothing was pushed"
    );
    assert!(recorded(&out).is_empty(), "{:?}", recorded(&out));
}

#[test]
fn a_dm_is_pushed_with_the_sender_as_reply_target() {
    let mut project = TestProject::with_name("push-dm");
    let out = project.work_dir().join("pushed.txt");
    attach_recorder(&mut project, "codex-a", &out);

    project
        .agent("someone")
        .send("@codex-a", "private ping")
        .assert_success();
    let got = recorded(&out);
    assert_eq!(got.len(), 1);
    let lines: Vec<&str> = got[0].lines().collect();
    assert_eq!(lines[1], "@someone");
    assert!(lines[2].contains("route=dm"));
}

#[test]
fn an_ambiguous_push_failure_keeps_the_session_and_reports_it() {
    let mut project = TestProject::with_name("push-failed");
    write_adapter(
        &project,
        "flaky",
        &["sh", "-c", "echo 'queue busy, try later' >&2; exit 3"],
    );
    attach(&mut project, "codex-a", "flaky", "flaky");

    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a are you there",
        "--format",
        "json",
    ]));
    let d = &sent["deliveries"][0];
    assert_eq!(d["ok"], false);
    assert!(
        d["error"].as_str().unwrap().contains("exited with 3"),
        "{d}"
    );
    assert!(
        d["error"].as_str().unwrap().contains("queue busy"),
        "stderr tail carried: {d}"
    );
    assert!(d.get("detached").is_none(), "not detached: {d}");

    let listed = json(
        &project
            .agent("codex-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert_eq!(listed["sessions"][0]["state"], "attached");
    assert_eq!(listed["sessions"][0]["occupancy"], "held");
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a again",
        "--format",
        "json",
    ]));
    assert_eq!(
        sent["deliveries"][0]["ok"], false,
        "pushed again, still failing"
    );
}

#[test]
fn a_session_gone_report_is_passed_on_but_never_evicts() {
    let mut project = TestProject::with_name("push-gone");
    write_adapter(
        &project,
        "gone",
        &["sh", "-c", "echo 'session does not exist' >&2; exit 66"],
    );
    let attached = attach(&mut project, "codex-a", "gone", "dead");
    let claim_id = attached["claim"]["id"].as_str().unwrap().to_string();

    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a are you there",
        "--format",
        "json",
    ]));
    let d = &sent["deliveries"][0];
    assert_eq!(d["ok"], false);
    assert_eq!(d["session_gone"], true, "the report is passed on: {d}");
    assert!(d.get("detached").is_none());

    // No adapter result evicts: the attachment and its claim stay until
    // SessionEnd or an explicit detach acts on the report.
    let listed = json(
        &project
            .agent("codex-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert_eq!(listed["sessions"][0]["state"], "attached");
    assert_eq!(listed["sessions"][0]["occupancy"], "held");
    assert!(project.active_claims().iter().any(|c| c["id"] == claim_id));
    let detached = json(&project.run_rite_with_env(
        &[
            "sessions",
            "detach",
            "--session",
            "dead",
            "--format",
            "json",
        ],
        None,
    ));
    assert_eq!(detached["released_claim"], claim_id);
}

#[test]
fn a_hung_push_times_out_kills_its_process_group_and_keeps_the_session() {
    let mut project = TestProject::with_name("push-timeout");
    let marker = project.work_dir().join("grandchild.txt");
    let script = format!("(sleep 8; echo late > {}) & sleep 30", marker.display());
    write_adapter(&project, "slow", &["sh", "-c", &script]);
    attach(&mut project, "codex-a", "slow", "slow");
    let started = std::time::Instant::now();
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a hello",
        "--format",
        "json",
    ]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(15),
        "send must not hang on a stuck push"
    );
    assert_eq!(sent["deliveries"][0]["ok"], false);
    assert!(
        sent["deliveries"][0]["error"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    assert!(
        sent["deliveries"][0].get("detached").is_none(),
        "a timeout is not proof the session is gone"
    );
    std::thread::sleep(std::time::Duration::from_secs(9));
    assert!(!marker.exists(), "grandchild survived the group kill");
}

#[test]
fn a_descendant_of_an_exited_adapter_is_killed_and_the_session_is_kept() {
    let mut project = TestProject::with_name("push-survivor");
    let marker = project.work_dir().join("late.txt");
    let script = format!("(sleep 3; echo late > {}) & exit 66", marker.display());
    write_adapter(&project, "forker", &["sh", "-c", &script]);
    attach(&mut project, "codex-a", "forker", "s");
    let started = std::time::Instant::now();
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a hi",
        "--format",
        "json",
    ]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "bounded by the drain timeout, not the child"
    );
    assert_eq!(sent["deliveries"][0]["session_gone"], true, "{sent}");
    let listed = json(
        &project
            .agent("codex-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert_eq!(
        listed["sessions"][0]["state"], "attached",
        "reports never evict"
    );
    std::thread::sleep(std::time::Duration::from_secs(4));
    assert!(!marker.exists(), "descendant survived the group kill");
}

#[test]
fn stream_sessions_and_unknown_harnesses_are_not_pushed() {
    let mut project = TestProject::with_name("push-kinds");
    project
        .agent("claude-a")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "claude",
            "--session",
            "mcp:1",
            "--kind",
            "stream",
        ])
        .assert_success();
    project
        .agent("other-a")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "tmux",
            "--session",
            "pane-1",
        ])
        .assert_success();
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a @other-a hi",
        "--format",
        "json",
    ]));
    assert!(sent.get("deliveries").is_none(), "{sent}");
}

#[test]
fn a_record_cannot_name_a_command_only_an_adapter_this_host_has() {
    let mut project = TestProject::with_name("push-unknown-adapter");
    project
        .agent("codex-a")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "custom",
            "--adapter",
            "not-here",
            "--session",
            "s",
        ])
        .assert_success();
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a hi",
        "--format",
        "json",
    ]));
    assert!(sent.get("deliveries").is_none(), "nothing ran: {sent}");
}

#[test]
fn no_hooks_suppresses_delivery_like_it_suppresses_hooks() {
    let mut project = TestProject::with_name("push-nohooks");
    let out = project.work_dir().join("pushed.txt");
    attach_recorder(&mut project, "codex-a", &out);
    project
        .agent("someone")
        .run(&["send", "general", "@codex-a quiet", "--no-hooks"])
        .assert_success();
    project
        .agent("someone")
        .run(&["send", "general", "@codex-a quiet too !nohooks"])
        .assert_success();
    assert!(recorded(&out).is_empty());
    project
        .agent("someone")
        .send("general", "@codex-a loud")
        .assert_success();
    assert_eq!(recorded(&out).len(), 1);
}

#[test]
fn the_adapter_runs_with_a_minimal_environment_in_the_local_dir() {
    let mut project = TestProject::with_name("push-env");
    let out = project.work_dir().join("env.txt");
    let script = format!(
        "printf '%s|%s|%s\\n' \"$SENDER_SECRET\" \"$PWD\" \"$RITE_FROM\" > {}",
        out.display()
    );
    write_adapter(&project, "env", &["sh", "-c", &script]);
    attach(&mut project, "codex-a", "env", "s");
    let status = std::process::Command::new(common::rite_bin())
        .args(["send", "--agent", "someone", "general", "@codex-a hi"])
        .env("RITE_DATA_DIR", project.data_path())
        .env("SENDER_SECRET", "hunter2")
        .current_dir(project.work_dir())
        .status()
        .unwrap();
    assert!(status.success());
    let line = std::fs::read_to_string(&out).unwrap();
    let parts: Vec<&str> = line.trim().split('|').collect();
    assert_eq!(parts[0], "", "sender environment must not leak: {line}");
    assert!(
        parts[1].ends_with("/local"),
        "runs in the data dir's local/: {line}"
    );
    assert_eq!(parts[2], "someone");
}

#[test]
fn only_the_newest_attachment_of_an_agent_is_pushed_and_the_old_one_is_retired() {
    let mut project = TestProject::with_name("push-replace-window");
    let out = project.work_dir().join("pushed.txt");
    let old_id = attach_recorder(&mut project, "codex-a", &out);
    // Simulate a crash between the successor's commit and the predecessor's
    // detach: a second attached record naming the first, both live.
    let succ = ulid::Ulid::new();
    let script = format!("printf 'SUCC %s\\n---\\n' \"$1\" >> {}", out.display());
    write_adapter(
        &project,
        "succ",
        &["sh", "-c", &script, "_", "{reply_target}"],
    );
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(), "attachment_id": succ.to_string(), "agent": "codex-a",
        "harness": "custom", "session": "sid-new", "kind": "push", "event": "attached",
        "replaces": old_id, "adapter": "succ"
    });
    let path = project.data_path().join("local/sessions.jsonl");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a once",
        "--format",
        "json",
    ]));
    assert_eq!(sent["deliveries"].as_array().unwrap().len(), 1, "{sent}");
    assert_eq!(sent["deliveries"][0]["session"], "sid-new");
    let got = recorded(&out);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("SUCC"));
    // A session command reconciles: the predecessor is retired.
    project
        .agent("codex-a")
        .run(&["sessions", "renew", "--attachment", &succ.to_string()])
        .assert_failure();
    let listed = json(
        &project
            .agent("codex-a")
            .run(&["sessions", "list", "--all", "--format", "json"]),
    );
    let old = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["attachment_id"] == old_id)
        .unwrap();
    assert_eq!(old["state"], "detached", "{listed}");
}

#[test]
fn owned_claims_survive_an_older_writers_ownerless_event_and_generic_release() {
    let mut project = TestProject::with_name("push-mixed-version");
    write_adapter(&project, "noop", &["true"]);
    let attached = attach(&mut project, "codex-a", "noop", "s");
    let claim_id = attached["claim"]["id"].as_str().unwrap().to_string();
    let claims_path = project.data_path().join("claims.jsonl");
    let mut record: serde_json::Value = std::fs::read_to_string(&claims_path)
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|c| c["id"] == claim_id)
        .unwrap();
    assert!(
        record["agent"].as_str().unwrap().starts_with("session:"),
        "{record}"
    );
    record["event"] = serde_json::json!("extended");
    record.as_object_mut().unwrap().remove("owner");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&claims_path)
            .unwrap();
        writeln!(f, "{record}").unwrap();
    }
    project.agent("codex-a").release_all().assert_success();
    let listed = json(
        &project
            .agent("codex-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert_eq!(listed["sessions"][0]["occupancy"], "held", "{listed}");
    let detached = json(&project.run_rite_with_env(
        &["sessions", "detach", "--session", "s", "--format", "json"],
        None,
    ));
    assert_eq!(detached["released_claim"], claim_id);
}

#[test]
fn the_codex_default_command_is_used_when_no_override_is_given() {
    let mut project = TestProject::with_name("push-codex-default");
    project
        .agent("codex-a")
        .run(&[
            "sessions",
            "attach",
            "--harness",
            "codex",
            "--session",
            "01a0-thread",
        ])
        .assert_success();
    let sent = project.run_rite_with_env(
        &["send", "general", "@codex-a hi", "--format", "json"],
        Some("someone"),
    );
    sent.assert_success();
    let sent = json(&sent);
    assert_eq!(sent["deliveries"][0]["agent"], "codex-a");
    assert_eq!(sent["deliveries"][0]["ok"], false);
}

#[test]
fn delivery_is_refused_while_local_state_is_tracked_by_sync() {
    let mut project = TestProject::with_name("push-tracked-local");
    let out = project.work_dir().join("pushed.txt");
    attach_recorder(&mut project, "codex-a", &out);
    let data = project.data_path().to_path_buf();
    project
        .agent("someone")
        .run(&["sync", "init"])
        .assert_success();
    git(&data, &["config", "commit.gpgsign", "false"]);
    assert!(
        git(&data, &["add", "-f", "local/sessions.jsonl"])
            .status
            .success()
    );
    assert!(git(&data, &["commit", "-q", "-m", "oops"]).status.success());

    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a hi",
        "--format",
        "json",
    ]));
    assert!(
        sent.get("deliveries").is_none(),
        "must not run adapters: {sent}"
    );
    assert!(recorded(&out).is_empty());
    let doctor = project.run_rite_with_env(&["doctor", "--format", "json"], Some("someone"));
    assert!(
        doctor.stdout_str().contains("local_state_untracked"),
        "{}",
        doctor.stdout_str()
    );
    assert!(
        doctor.stdout_str().contains("Host-local state is tracked"),
        "{}",
        doctor.stdout_str()
    );

    project
        .agent("someone")
        .run(&["sync", "commit", "-m", "fix"])
        .assert_success();
    project
        .agent("someone")
        .send("general", "@codex-a again")
        .assert_success();
    assert_eq!(recorded(&out).len(), 1);
}

#[test]
fn sync_pull_refuses_a_remote_that_tracks_local_state() {
    let mut a = TestProject::with_name("push-quarantine-a");
    let mut b = TestProject::with_name("push-quarantine-b");
    let marker = b.work_dir().join("pwned.txt");
    let bare = b.work_dir().join("remote.git");
    assert!(
        git(
            b.work_dir(),
            &["init", "--bare", "-q", bare.to_str().unwrap()]
        )
        .status
        .success()
    );
    for p in [&mut a, &mut b] {
        p.agent("x").run(&["sync", "init"]).assert_success();
        let d = p.data_path().to_path_buf();
        git(&d, &["config", "commit.gpgsign", "false"]);
        git(&d, &["config", "user.email", "t@t"]);
        git(&d, &["config", "user.name", "t"]);
        git(&d, &["remote", "add", "origin", bare.to_str().unwrap()]);
    }
    let a_data = a.data_path().to_path_buf();
    std::fs::create_dir_all(a_data.join("local")).unwrap();
    std::fs::write(
        a_data.join("local/adapters.json"),
        format!(r#"{{"codex":["sh","-c","touch {}"]}}"#, marker.display()),
    )
    .unwrap();
    std::fs::write(
        a_data.join("local/sessions.jsonl"),
        format!(
            "{{\"ts\":\"{}\",\"attachment_id\":\"{}\",\"agent\":\"victim\",\"harness\":\"codex\",\"session\":\"evil\",\"kind\":\"push\",\"event\":\"attached\"}}\n",
            chrono::Utc::now().to_rfc3339(),
            ulid::Ulid::new()
        ),
    )
    .unwrap();
    assert!(git(&a_data, &["add", "-f", "local"]).status.success());
    assert!(
        git(&a_data, &["commit", "-q", "-m", "smuggle"])
            .status
            .success()
    );
    let refused = a.agent("x").run(&["sync", "push"]);
    refused.assert_failure();
    assert!(
        refused.stderr_str().contains("host-local state is tracked"),
        "{}",
        refused.stderr_str()
    );
    assert!(
        git(&a_data, &["push", "-q", "origin", "HEAD:main"])
            .status
            .success(),
        "raw git push"
    );

    // B refuses the merge outright: nothing from the remote's local/ ever
    // touches B's working tree, so there is no window for a send to act on.
    let b_data = b.data_path().to_path_buf();
    let before = git(&b_data, &["rev-parse", "HEAD"]).stdout;
    let pulled = b.agent("x").run(&["sync", "pull"]);
    pulled.assert_failure();
    assert!(
        pulled.stderr_str().contains("refusing to merge"),
        "{}",
        pulled.stderr_str()
    );
    assert_eq!(
        git(&b_data, &["rev-parse", "HEAD"]).stdout,
        before,
        "HEAD unchanged"
    );
    assert!(
        !git(&b_data, &["ls-files", "--error-unmatch", "--", "local"])
            .status
            .success()
    );
    assert!(
        !b_data.join("local/sessions.jsonl").exists(),
        "imported log not in place"
    );
    assert!(
        !b_data.join("local/adapters.json").exists(),
        "imported table not in place"
    );
    b.agent("someone")
        .send("general", "@victim hi")
        .assert_success();
    assert!(!marker.exists(), "imported adapter executed");
}

#[test]
fn sync_push_refuses_a_range_whose_history_touched_local_state() {
    let mut a = TestProject::with_name("push-history-a");
    let bare = a.work_dir().join("remote.git");
    assert!(
        git(
            a.work_dir(),
            &["init", "--bare", "-q", bare.to_str().unwrap()]
        )
        .status
        .success()
    );
    a.agent("x").run(&["sync", "init"]).assert_success();
    let d = a.data_path().to_path_buf();
    git(&d, &["config", "commit.gpgsign", "false"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["remote", "add", "origin", bare.to_str().unwrap()]);
    // Add local state, then delete it: the tip is clean, the history is not.
    std::fs::create_dir_all(d.join("local")).unwrap();
    std::fs::write(d.join("local/sessions.jsonl"), "{}\n").unwrap();
    assert!(
        git(&d, &["add", "-f", "local/sessions.jsonl"])
            .status
            .success()
    );
    assert!(git(&d, &["commit", "-q", "-m", "add"]).status.success());
    assert!(
        git(&d, &["rm", "-q", "--cached", "local/sessions.jsonl"])
            .status
            .success()
    );
    assert!(git(&d, &["commit", "-q", "-m", "remove"]).status.success());
    let out = a.agent("x").run(&["sync", "push"]);
    out.assert_failure();
    assert!(
        out.stderr_str().contains("touch host-local state"),
        "{}",
        out.stderr_str()
    );
    let remote_has = git(
        a.work_dir(),
        &[
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-parse",
            "--verify",
            "--quiet",
            "main",
        ],
    );
    assert!(!remote_has.status.success(), "nothing reached the remote");
}

#[test]
fn a_descendant_that_escapes_the_group_cannot_evict_the_session() {
    let mut project = TestProject::with_name("push-setsid");
    let marker = project.work_dir().join("escaped.txt");
    let script = format!(
        "setsid sh -c 'sleep 4; touch {}' >/dev/null 2>&1 & exit 66",
        marker.display()
    );
    write_adapter(&project, "escaper", &["sh", "-c", &script]);
    let attached = attach(&mut project, "codex-a", "escaper", "s");
    let started = std::time::Instant::now();
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@codex-a hi",
        "--format",
        "json",
    ]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(6),
        "bounded"
    );
    let d = &sent["deliveries"][0];
    assert_eq!(d["ok"], false);
    assert_eq!(
        d["session_gone"], true,
        "the report is still passed on: {d}"
    );
    let listed = json(
        &project
            .agent("codex-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert_eq!(
        listed["sessions"][0]["attachment_id"],
        attached["attachment_id"]
    );
    assert_eq!(
        listed["sessions"][0]["occupancy"], "held",
        "an escaped process cannot cause a false detach"
    );
}

#[test]
fn sync_push_refuses_a_treesame_side_branch_that_touched_local_state() {
    let mut a = TestProject::with_name("push-treesame-a");
    let bare = a.work_dir().join("remote.git");
    assert!(
        git(
            a.work_dir(),
            &["init", "--bare", "-q", bare.to_str().unwrap()]
        )
        .status
        .success()
    );
    a.agent("x").run(&["sync", "init"]).assert_success();
    let d = a.data_path().to_path_buf();
    git(&d, &["config", "commit.gpgsign", "false"]);
    git(&d, &["config", "user.email", "t@t"]);
    git(&d, &["config", "user.name", "t"]);
    git(&d, &["remote", "add", "origin", bare.to_str().unwrap()]);
    // Side branch adds then deletes local state; merged back, the merge is
    // TREESAME to main for that path, which path-limited git log prunes.
    assert!(git(&d, &["checkout", "-q", "-b", "side"]).status.success());
    std::fs::create_dir_all(d.join("local")).unwrap();
    std::fs::write(d.join("local/sessions.jsonl"), "{}\n").unwrap();
    assert!(
        git(&d, &["add", "-f", "local/sessions.jsonl"])
            .status
            .success()
    );
    assert!(git(&d, &["commit", "-q", "-m", "add"]).status.success());
    assert!(
        git(&d, &["rm", "-q", "--cached", "local/sessions.jsonl"])
            .status
            .success()
    );
    assert!(git(&d, &["commit", "-q", "-m", "remove"]).status.success());
    assert!(git(&d, &["checkout", "-q", "main"]).status.success());
    assert!(
        git(&d, &["merge", "-q", "--no-ff", "-m", "merge side", "side"])
            .status
            .success()
    );
    // And from a non-main checkout, since push always sends main.
    assert!(git(&d, &["checkout", "-q", "side"]).status.success());
    let out = a.agent("x").run(&["sync", "push"]);
    out.assert_failure();
    assert!(
        out.stderr_str().contains("touch host-local state"),
        "{}",
        out.stderr_str()
    );
    let remote_has = git(
        a.work_dir(),
        &[
            "--git-dir",
            bare.to_str().unwrap(),
            "rev-parse",
            "--verify",
            "--quiet",
            "main",
        ],
    );
    assert!(!remote_has.status.success(), "nothing reached the remote");
}

#[test]
fn case_variants_of_local_are_refused_by_pull_and_push() {
    // On a case-insensitive filesystem LOCAL/ is local/. The guards must
    // catch the variant even where the filesystem would not alias it.
    let mut a = TestProject::with_name("push-case-a");
    let mut b = TestProject::with_name("push-case-b");
    let marker = b.work_dir().join("pwned.txt");
    let bare = b.work_dir().join("remote.git");
    assert!(
        git(
            b.work_dir(),
            &["init", "--bare", "-q", bare.to_str().unwrap()]
        )
        .status
        .success()
    );
    for p in [&mut a, &mut b] {
        p.agent("x").run(&["sync", "init"]).assert_success();
        let d = p.data_path().to_path_buf();
        git(&d, &["config", "commit.gpgsign", "false"]);
        git(&d, &["config", "user.email", "t@t"]);
        git(&d, &["config", "user.name", "t"]);
        git(&d, &["remote", "add", "origin", bare.to_str().unwrap()]);
    }
    let a_data = a.data_path().to_path_buf();
    std::fs::create_dir_all(a_data.join("LOCAL")).unwrap();
    std::fs::write(
        a_data.join("LOCAL/adapters.json"),
        format!(r#"{{"codex":["sh","-c","touch {}"]}}"#, marker.display()),
    )
    .unwrap();
    std::fs::write(
        a_data.join("LOCAL/sessions.jsonl"),
        format!(
            "{{\"ts\":\"{}\",\"attachment_id\":\"{}\",\"agent\":\"victim\",\"harness\":\"codex\",\"session\":\"evil\",\"kind\":\"push\",\"event\":\"attached\"}}\n",
            chrono::Utc::now().to_rfc3339(),
            ulid::Ulid::new()
        ),
    )
    .unwrap();
    assert!(git(&a_data, &["add", "-f", "LOCAL"]).status.success());
    assert!(
        git(&a_data, &["commit", "-q", "-m", "smuggle upper"])
            .status
            .success()
    );
    let refused = a.agent("x").run(&["sync", "push"]);
    refused.assert_failure();
    assert!(
        refused.stderr_str().contains("host-local state"),
        "{}",
        refused.stderr_str()
    );
    assert!(
        git(&a_data, &["push", "-q", "origin", "HEAD:main"])
            .status
            .success(),
        "raw git push"
    );

    let b_data = b.data_path().to_path_buf();
    let pulled = b.agent("x").run(&["sync", "pull"]);
    pulled.assert_failure();
    assert!(
        pulled.stderr_str().contains("refusing to merge"),
        "{}",
        pulled.stderr_str()
    );
    assert!(!b_data.join("LOCAL").exists() && !b_data.join("local/adapters.json").exists());
    // A tracked case variant on this host refuses delivery too.
    std::fs::create_dir_all(b_data.join("Local")).unwrap();
    std::fs::write(b_data.join("Local/note"), "x").unwrap();
    assert!(git(&b_data, &["add", "-f", "Local/note"]).status.success());
    assert!(
        git(&b_data, &["commit", "-q", "-m", "variant"])
            .status
            .success()
    );
    let out = b.work_dir().join("pushed.txt");
    attach_recorder(&mut b, "codex-b", &out);
    let sent =
        json(
            &b.agent("someone")
                .run(&["send", "general", "@codex-b hi", "--format", "json"]),
        );
    assert!(
        sent.get("deliveries").is_none(),
        "tracked case variant must refuse delivery: {sent}"
    );
    assert!(!marker.exists());
}
