//! `rite channel`: the Claude Code channel server, driven over pipes the way
//! Claude Code drives it.

mod common;

use common::TestProject;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

struct Server {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<String>,
}

fn start(project: &TestProject, agent: &str) -> Server {
    start_with_env(project, agent, &[])
}

/// Like [`start`], with extra environment for the server and its children.
fn start_with_env(project: &TestProject, agent: &str, env: &[(&str, &str)]) -> Server {
    let mut child = Command::new(common::rite_bin())
        .args(["channel", "--agent", agent, "--renew", "1m"])
        .env("RITE_DATA_DIR", project.data_path())
        .envs(env.iter().copied())
        .current_dir(project.work_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rite channel");
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    Server {
        child,
        stdin,
        lines: rx,
    }
}

impl Server {
    fn send(&mut self, v: serde_json::Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }
    fn next(&self, what: &str) -> serde_json::Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(15))
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }
    /// Skip notifications until a response with this id arrives.
    fn response(&self, id: u64) -> serde_json::Value {
        loop {
            let v = self.next(&format!("response {id}"));
            if v["id"] == id {
                return v;
            }
        }
    }
    /// The next channel event whose msg_id matches, skipping others.
    fn event_for(&self, msg_id: &str) -> serde_json::Value {
        loop {
            let v = self.next(&format!("event for {msg_id}"));
            if v["method"] == "notifications/claude/channel"
                && v["params"]["meta"]["msg_id"] == msg_id
            {
                return v;
            }
        }
    }
}

fn json(out: &common::RiteOutput) -> serde_json::Value {
    serde_json::from_str(&out.stdout_str()).unwrap()
}

#[test]
fn channel_serves_mentions_and_replies_and_owns_the_session() {
    let mut project = TestProject::with_name("channel-roundtrip");
    let mut server = start(&project, "claude-a");

    // MCP handshake.
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}));
    let init = server.response(1);
    assert!(
        init["result"]["capabilities"]["experimental"]["claude/channel"].is_object(),
        "{init}"
    );
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("reply tool")
    );
    // Before initialized, the reply tool must refuse: no occupancy yet.
    server.send(serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
        "name":"reply","arguments":{"target":"general","text":"@someone too early","reply_to":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}}}));
    let early = server.response(9);
    assert_eq!(early["result"]["isError"], true, "{early}");
    let hist = json(
        &project
            .agent("someone")
            .run(&["history", "general", "-n", "5", "--format", "json"]),
    );
    assert!(
        hist["messages"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "nothing written to the bus: {hist}"
    );

    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    server.send(serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let tools = server.response(2);
    assert_eq!(tools["result"]["tools"][0]["name"], "reply");

    // The process is the live session: attached as a stream, claim held.
    let listed = json(
        &project
            .agent("claude-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert_eq!(listed["sessions"][0]["kind"], "stream", "{listed}");
    assert_eq!(listed["sessions"][0]["harness"], "claude");
    assert_eq!(listed["sessions"][0]["occupancy"], "held");
    let attachment = listed["sessions"][0]["attachment_id"]
        .as_str()
        .unwrap()
        .to_string();

    // A mention on the bus becomes a channel event. The watcher arms
    // asynchronously, so probe until the first event arrives.
    let mut n = 0;
    let (sent, ev) = loop {
        n += 1;
        let body = format!("@claude-a hello there {n}");
        let sent = json(
            &project
                .agent("someone")
                .run(&["send", "general", &body, "--format", "json"]),
        );
        if let Ok(line) = server.lines.recv_timeout(Duration::from_millis(700)) {
            let ev: serde_json::Value = serde_json::from_str(&line).unwrap();
            if ev["method"] == "notifications/claude/channel" {
                // The first event may be for an earlier probe; align on this one.
                let ev = if ev["params"]["meta"]["msg_id"] == sent["id"] {
                    ev
                } else {
                    server.event_for(sent["id"].as_str().unwrap())
                };
                break (sent, ev);
            }
        }
        assert!(n < 20, "no channel event after {n} probes");
    };
    assert_eq!(
        ev["params"]["content"],
        format!("@claude-a hello there {n}")
    );
    assert_eq!(ev["params"]["meta"]["from_agent"], "someone");
    assert_eq!(ev["params"]["meta"]["channel_name"], "general");
    assert_eq!(ev["params"]["meta"]["reply_target"], "general");
    assert_eq!(ev["params"]["meta"]["route"], "mention");

    // A DM routes with the sender as reply target.
    let dm_sent =
        json(
            &project
                .agent("someone")
                .run(&["send", "@claude-a", "private", "--format", "json"]),
        );
    let dm = server.event_for(dm_sent["id"].as_str().unwrap());
    assert_eq!(dm["params"]["meta"]["route"], "dm");
    assert_eq!(dm["params"]["meta"]["reply_target"], "@someone");

    // The reply tool answers on the bus, anchored.
    server.send(serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
        "name":"reply","arguments":{"target":"general","text":"@someone hi back","reply_to": sent["id"]}}}));
    let r = server.response(3);
    assert_eq!(r["result"]["isError"], false, "{r}");
    let thread = json(&project.run_rite_with_env(
        &[
            "history",
            "--thread",
            sent["id"].as_str().unwrap(),
            "--format",
            "json",
        ],
        Some("someone"),
    ));
    let msgs = thread["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2, "{thread}");
    assert_eq!(msgs[1]["agent"], "claude-a");
    assert_eq!(msgs[1]["reply_to"], sent["id"]);

    // Unknown methods with an id get a JSON-RPC error.
    server.send(serde_json::json!({"jsonrpc":"2.0","id":4,"method":"nope"}));
    assert_eq!(server.response(4)["error"]["code"], -32601);

    // Claude closes the pipe: the session detaches and the claim is released.
    drop(server.stdin);
    let status = server.child.wait().unwrap();
    assert!(status.success());
    let listed = json(
        &project
            .agent("claude-a")
            .run(&["sessions", "list", "--all", "--format", "json"]),
    );
    let s = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["attachment_id"] == attachment)
        .unwrap();
    assert_eq!(s["state"], "detached");
    let now = chrono::Utc::now();
    let held = project.active_claims().into_iter().any(|c| {
        c["patterns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "agent://claude-a")
            && c["expires_at"]
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t > now)
    });
    assert!(!held, "claim released");
}

#[test]
fn channel_exits_instead_of_serving_when_the_identity_is_held() {
    let mut project = TestProject::with_name("channel-held");
    project
        .agent("other")
        .claim(&["agent://claude-a"])
        .assert_success();
    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    // Occupancy is taken only at initialized; it cannot be, so the server
    // must stop rather than stream and reply beside the holder.
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    // A mention that lands while attach is still deciding must not come through.
    project
        .agent("someone")
        .send("general", "@claude-a racing the attach")
        .assert_success();
    // A reply attempt from the loser must be refused, not written.
    server.send(serde_json::json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{
        "name":"reply","arguments":{"target":"general","text":"@someone from the loser","reply_to":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}}}));
    let status = server.child.wait().unwrap();
    assert!(!status.success(), "must exit non-zero: {status:?}");
    assert!(
        server.lines.recv_timeout(Duration::from_secs(1)).is_err(),
        "nothing streamed"
    );
    let listed = json(
        &project
            .agent("claude-a")
            .run(&["sessions", "list", "--all", "--format", "json"]),
    );
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] != "attached"),
        "no live attachment: {listed}"
    );
    // The competing claim was never touched.
    assert!(
        project
            .active_claims()
            .iter()
            .any(|c| c["agent"] == "other")
    );
    let msgs = json(
        &project
            .agent("someone")
            .run(&["history", "general", "-n", "5", "--format", "json"]),
    );
    assert!(
        msgs["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["agent"] != "claude-a"),
        "the loser must not have written as claude-a: {msgs}"
    );
}

#[test]
fn channel_serves_without_occupancy_only_when_asked() {
    let mut project = TestProject::with_name("channel-noattach");
    project
        .agent("other")
        .claim(&["agent://claude-a"])
        .assert_success();
    let mut child = Command::new(common::rite_bin())
        .args(["channel", "--agent", "claude-a", "--no-attach"])
        .env("RITE_DATA_DIR", project.data_path())
        .current_dir(project.work_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"ping"}}"#).unwrap();
    stdin.flush().unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let mut saw_ping = false;
    for line in out.lines().map_while(Result::ok).take(2) {
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        if v["id"] == 2 {
            saw_ping = true;
        }
    }
    assert!(saw_ping, "serves after initialized with --no-attach");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let listed = json(
        &project
            .agent("claude-a")
            .run(&["sessions", "list", "--all", "--format", "json"]),
    );
    assert!(
        listed["sessions"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "{listed}"
    );
}

#[test]
fn occupancy_is_taken_only_after_initialized_and_the_stream_is_armed() {
    let mut project = TestProject::with_name("channel-ready");
    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    std::thread::sleep(Duration::from_millis(500));
    let listed = json(
        &project
            .agent("claude-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    assert!(
        listed["sessions"].as_array().unwrap().is_empty(),
        "no occupancy before initialized: {listed}"
    );

    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let listed = json(
            &project
                .agent("claude-a")
                .run(&["sessions", "list", "--format", "json"]),
        );
        if listed["sessions"].as_array().unwrap().len() == 1 {
            assert_eq!(listed["sessions"][0]["occupancy"], "held");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never attached: {listed}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // With occupancy held, a message is deliverable right away.
    let sent = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a now",
        "--format",
        "json",
    ]));
    let ev = server.event_for(sent["id"].as_str().unwrap());
    assert_eq!(ev["params"]["content"], "@claude-a now");
    drop(server.stdin);
    assert!(server.child.wait().unwrap().success());
}

/// The channels directory is replaced under a serving channel. Its mention
/// stream ends, so the server exits, detaches exactly its own attachment,
/// and the identity is free again rather than held over a silent stream.
#[test]
fn channel_exits_and_releases_occupancy_when_the_channels_directory_is_replaced() {
    let mut project = TestProject::with_name("channel-dir-replaced");
    let channels = project.data_path().join("channels");
    project
        .agent("someone")
        .send("general", "hello before @claude-a")
        .assert_success();
    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let attached = |project: &mut TestProject| {
        let listed = json(
            &project
                .agent("claude-a")
                .run(&["sessions", "list", "--all", "--format", "json"]),
        );
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["state"] == "attached")
    };
    let holds_identity = |project: &TestProject| {
        project.active_claims().iter().any(|c| {
            c["patterns"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x == "agent://claude-a"))
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !(attached(&mut project) && holds_identity(&project)) {
        assert!(
            std::time::Instant::now() < deadline,
            "occupancy never taken"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    std::fs::rename(&channels, project.data_path().join("channels.old")).unwrap();
    std::fs::create_dir(&channels).unwrap();

    let status = server.child.wait().unwrap();
    assert!(!status.success(), "must exit non-zero: {status:?}");
    assert!(
        !holds_identity(&project),
        "occupancy still held after the stream ended: {:?}",
        project.active_claims()
    );
    assert!(!attached(&mut project), "an attachment survived the exit");
}

/// A reply into a channel whose hook waits for its spawn to exit used to
/// hold the server's lifecycle for as long as that spawn lived, and with it
/// shutdown and occupancy. The reply now returns unconfirmed after a bound,
/// and the server can still exit and detach promptly.
#[test]
fn a_reply_held_by_a_slow_hook_does_not_hold_occupancy() {
    let mut project = TestProject::with_name("channel-slow-hook");
    let cwd = project.work_dir().display().to_string();
    project
        .run_rite_with_env(
            &[
                "hooks",
                "add",
                "--channel",
                "general",
                "--claim",
                "hook://slow",
                "--release-on-exit",
                "--cwd",
                &cwd,
                "--",
                "sleep",
                "50",
            ],
            Some("ops"),
        )
        .assert_success();
    let id = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a please answer",
        "--format",
        "json",
        "--no-hooks",
    ]))["id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let holds_identity = |project: &TestProject| {
        project.active_claims().iter().any(|c| {
            c["patterns"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x == "agent://claude-a"))
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !holds_identity(&project) {
        assert!(
            std::time::Instant::now() < deadline,
            "occupancy never taken"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let started = std::time::Instant::now();
    server.send(serde_json::json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
        "name":"reply","arguments":{"target":"general","text":"@someone answering","reply_to":id}}}));
    let reply = loop {
        let v = server
            .lines
            .recv_timeout(Duration::from_secs(45))
            .expect("reply result within the bound");
        let v: serde_json::Value = serde_json::from_str(&v).unwrap();
        if v["id"] == 7 {
            break v;
        }
    };
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let result: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(
        result["id"].as_str().is_some_and(|s| s.len() == 26),
        "the appended reply is identified from the bus: {text}"
    );
    assert!(
        result["note"]
            .as_str()
            .is_some_and(|n| n.contains("hook it triggered is still running")),
        "{text}"
    );
    assert!(started.elapsed() < Duration::from_secs(45));

    // The reply itself landed on the bus before the hook ran.
    let msgs = json(
        &project
            .agent("someone")
            .run(&["history", "general", "-n", "5", "--format", "json"]),
    );
    assert!(
        msgs["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["agent"] == "claude-a"),
        "{msgs}"
    );

    // Closing stdin ends the server promptly; occupancy is released.
    drop(server.stdin);
    let status = server.child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(
        started.elapsed() < Duration::from_secs(48),
        "teardown waited on the hook: {:?}",
        started.elapsed()
    );
    assert!(!holds_identity(&project), "occupancy still held");
}

/// A reply whose `rite send` stalls before the append (here: the destination
/// channel's lock is held) is ended at the deadline, so it can never write
/// as this agent after the identity has moved on. The tool reports that
/// nothing is on the bus.
#[test]
fn a_reply_stalled_before_the_append_is_ended_and_never_writes_later() {
    use fs2::FileExt;

    let mut project = TestProject::with_name("channel-stalled-send");
    let id = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a please answer",
        "--format",
        "json",
    ]))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let channel_file = project.data_path().join("channels/general.jsonl");

    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let holds_identity = |project: &TestProject| {
        project.active_claims().iter().any(|c| {
            c["patterns"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x == "agent://claude-a"))
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !holds_identity(&project) {
        assert!(
            std::time::Instant::now() < deadline,
            "occupancy never taken"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // An identical reply already on the bus must not vouch for the stalled
    // one below.
    server.send(serde_json::json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
        "name":"reply","arguments":{"target":"general","text":"@someone late answer","reply_to":id}}}));
    let first = server.response(6);
    assert_eq!(first["result"]["isError"], false, "{first}");

    // Hold the channel's exclusive lock: the send cannot append.
    let lock = std::fs::OpenOptions::new()
        .append(true)
        .open(&channel_file)
        .unwrap();
    lock.lock_exclusive().unwrap();

    server.send(serde_json::json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
        "name":"reply","arguments":{"target":"general","text":"@someone late answer","reply_to":id}}}));
    let reply = loop {
        let v = server
            .lines
            .recv_timeout(Duration::from_secs(45))
            .expect("reply result within the bound");
        let v: serde_json::Value = serde_json::from_str(&v).unwrap();
        if v["id"] == 7 {
            break v;
        }
    };
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("reply not sent"), "{text}");

    // The identity moves on, then the lock is released: nothing appears.
    drop(server.stdin);
    let status = server.child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    assert!(!holds_identity(&project));
    lock.unlock().unwrap();
    drop(lock);
    std::thread::sleep(Duration::from_millis(500));
    let msgs = json(
        &project
            .agent("someone")
            .run(&["history", "general", "-n", "10", "--format", "json"]),
    );
    let from_a = msgs["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["agent"] == "claude-a")
        .count();
    assert_eq!(from_a, 1, "the ended send wrote after all: {msgs}");
}

/// `sessions attach --replace` re-tags the claim to its successor without
/// telling the old server. The old server must notice on its next delivery
/// or reply, not at its next renewal: it emits nothing more, refuses to
/// reply, exits, and leaves the successor's occupancy untouched.
#[test]
fn a_replaced_channel_stops_delivering_and_replying_at_once() {
    let mut project = TestProject::with_name("channel-replaced");
    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let attached = |project: &mut TestProject| -> Option<String> {
        let listed = json(
            &project
                .agent("claude-a")
                .run(&["sessions", "list", "--format", "json"]),
        );
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["state"] == "attached")
            .and_then(|s| s["attachment_id"].as_str().map(String::from))
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let old = loop {
        if let Some(id) = attached(&mut project) {
            break id;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "occupancy never taken"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    // Prove the stream is armed: one mention comes through.
    let first = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a before takeover",
        "--format",
        "json",
    ]))["id"]
        .as_str()
        .unwrap()
        .to_string();
    server.event_for(&first);

    // Take the identity over from outside, as a launcher would.
    let successor = json(&project.agent("claude-a").run(&[
        "sessions",
        "attach",
        "--harness",
        "claude",
        "--session",
        "successor-session",
        "--kind",
        "stream",
        "--replace",
        &old,
        "--format",
        "json",
    ]));
    let successor_id = successor["attachment_id"].as_str().unwrap().to_string();
    assert_ne!(successor_id, old);

    let late = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a after takeover",
        "--format",
        "json",
    ]))["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Nothing more is delivered, and the server ends.
    let status = server.child.wait().unwrap();
    assert!(
        !status.success(),
        "must exit non-zero after losing occupancy: {status:?}"
    );
    while let Ok(line) = server.lines.recv_timeout(Duration::from_millis(200)) {
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_ne!(
            v["params"]["meta"]["msg_id"],
            late.as_str(),
            "delivered after takeover: {line}"
        );
    }

    // The successor still holds the identity and its attachment is live.
    assert_eq!(
        attached(&mut project).as_deref(),
        Some(successor_id.as_str())
    );
    assert!(
        project.active_claims().iter().any(|c| {
            c["patterns"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x == "agent://claude-a"))
        }),
        "the successor's claim was released by the retired server"
    );
}

/// Deliveries are written under the host-wide claims lock, so a client that
/// stops reading must not hold that lock for long: the write is bounded,
/// and a stalled stdout ends the server, which releases occupancy.
#[test]
fn a_client_that_stops_reading_cannot_hold_the_claims_lock() {
    let mut project = TestProject::with_name("channel-stalled-stdout");
    // Like `start`, but nobody drains stdout: the pipe fills up.
    let mut child = Command::new(common::rite_bin())
        .args(["channel", "--agent", "claude-a", "--renew", "1m"])
        .env("RITE_DATA_DIR", project.data_path())
        .current_dir(project.work_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rite channel");
    let mut stdin = child.stdin.take().unwrap();
    let _unread = child.stdout.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#).unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    let holds_identity = |project: &TestProject| {
        project.active_claims().iter().any(|c| {
            c["patterns"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x == "agent://claude-a"))
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !holds_identity(&project) {
        assert!(
            std::time::Instant::now() < deadline,
            "occupancy never taken"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // Far more than a pipe buffer of events, none of them read.
    let filler = "x".repeat(8 * 1024);
    let started = std::time::Instant::now();
    for i in 0..24 {
        project
            .agent("someone")
            .send("general", &format!("@claude-a {i} {filler}"))
            .assert_success();
        // Every send takes the claims lock itself (hooks); if the server
        // were holding it across a blocked write, this would hang.
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "sends are being held up"
        );
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
    }
    let exit_deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            std::time::Instant::now() < exit_deadline,
            "server did not stop"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(!status.success(), "{status:?}");
    assert!(!holds_identity(&project), "occupancy still held");
}

/// The activation flush shares one write deadline. A client that drains
/// one buffered event just before each per-line deadline would otherwise
/// hold the host-wide claims lock for the whole backlog; with one bound
/// the server gives up, and occupancy, within about that bound.
#[test]
fn a_slow_drain_of_the_activation_backlog_is_bounded_as_a_whole() {
    use fs2::FileExt;
    use std::io::Read;

    let mut project = TestProject::with_name("channel-slow-flush");
    // Hold the claims lock: the stream arms on initialized, then the attach
    // stalls on this lock, and everything that lands meanwhile is buffered.
    let claims_file = project.data_path().join("claims.jsonl");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&claims_file)
        .unwrap();
    lock.lock_exclusive().unwrap();

    let mut child = Command::new(common::rite_bin())
        .args(["channel", "--agent", "claude-a", "--renew", "1m"])
        .env("RITE_DATA_DIR", project.data_path())
        .current_dir(project.work_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn rite channel");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#).unwrap();
    stdin.flush().unwrap();
    // The initialize response, so the pipe starts empty.
    let mut first = [0u8; 4096];
    let n = stdout.read(&mut first).unwrap();
    assert!(
        std::str::from_utf8(&first[..n])
            .unwrap()
            .contains("\"id\":1")
    );
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    // The stream is armed before the attach starts, and the attach is stuck.
    std::thread::sleep(Duration::from_secs(1));

    // Without hooks a send never touches the claims lock. Each event is
    // larger than a pipe buffer, so no write below completes until the
    // reader takes the one before it.
    let filler = "x".repeat(64 * 1024);
    for i in 0..12 {
        project
            .agent("someone")
            .run(&[
                "send",
                "general",
                &format!("@claude-a {i} {filler}"),
                "--no-hooks",
            ])
            .assert_success();
    }
    std::thread::sleep(Duration::from_millis(500));

    // A reader that drains one event's worth every 1.5s: each line alone
    // fits a per-write deadline, the backlog as a whole does not.
    let drain = std::thread::spawn(move || {
        let mut chunk = vec![0u8; 64 * 1024 + 4096];
        loop {
            std::thread::sleep(Duration::from_millis(1500));
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let started = std::time::Instant::now();
    lock.unlock().unwrap();
    drop(lock);

    let exit_deadline = started + Duration::from_secs(12);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            std::time::Instant::now() < exit_deadline,
            "the flush is earning a deadline per line"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(!status.success(), "{status:?}");
    let holds_identity = project.active_claims().iter().any(|c| {
        c["patterns"]
            .as_array()
            .is_some_and(|p| p.iter().any(|x| x == "agent://claude-a"))
    });
    assert!(!holds_identity, "occupancy still held");
    let _ = drain.join();
}

/// A message id is used once. The hidden `rite send --id` carries the id a
/// reply minted for itself; a reused id is refused at the append, in the
/// destination and in every other channel, so a follower that saw the
/// first message can never be made to swallow a second one under it.
#[test]
fn a_reused_message_id_is_refused_at_the_append() {
    let mut project = TestProject::with_name("channel-reused-id");
    let id = ulid::Ulid::new().to_string();
    project
        .agent("someone")
        .run(&["send", "general", "@claude-a first", "--id", &id])
        .assert_success();

    let again = project
        .agent("someone")
        .run(&["send", "general", "@claude-a second", "--id", &id]);
    again.assert_failure();
    assert!(
        again.stderr_contains("already in #general"),
        "{}",
        again.stderr_str()
    );

    let elsewhere =
        project
            .agent("someone")
            .run(&["send", "other", "@claude-a third", "--id", &id]);
    elsewhere.assert_failure();
    assert!(
        elsewhere.stderr_contains("already in #general"),
        "{}",
        elsewhere.stderr_str()
    );

    let bodies: Vec<String> = project
        .channel_messages("general")
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(bodies, vec!["@claude-a first".to_string()]);
    assert!(project.channel_messages("other").is_empty());
}

/// A caller-supplied id is fenced store-wide: one `--id` send sweeps and
/// appends at a time, so two of them cannot both pass the sweep and land
/// the same id in two channels. The fence is a lock the test can hold.
#[test]
fn caller_supplied_ids_are_fenced_store_wide() {
    use fs2::FileExt;

    let project = TestProject::with_name("channel-id-fence");
    let local = project.data_path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    let fence = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(local.join("send-id.lock"))
        .unwrap();
    fence.lock_exclusive().unwrap();

    let send = |channel: &str, id: &str| {
        Command::new(common::rite_bin())
            .args([
                "send",
                channel,
                "@claude-a fenced",
                "--id",
                id,
                "--no-hooks",
                "--agent",
                "someone",
            ])
            .env("RITE_DATA_DIR", project.data_path())
            .current_dir(project.work_dir())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn rite send")
    };
    let id = ulid::Ulid::new().to_string();
    let mut held = send("general", &id);
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        held.try_wait().unwrap().is_none(),
        "a --id send went ahead without the fence"
    );
    fence.unlock().unwrap();
    drop(fence);
    assert!(held.wait().unwrap().success());

    // Eight at once, one id, eight channels: exactly one lands.
    let id = ulid::Ulid::new().to_string();
    let names: Vec<String> = (0..8).map(|i| format!("race-{i}")).collect();
    let mut children: Vec<_> = names.iter().map(|c| send(c, &id)).collect();
    let successes = children
        .iter_mut()
        .map(|c| c.wait().unwrap().success())
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1);
    let landed: usize = names
        .iter()
        .map(|c| project.channel_messages(c).len())
        .sum();
    assert_eq!(landed, 1);
}

/// Attach `agent` as a `pull` placeholder, the way a launcher hook does,
/// under the Claude process `host` (`RITE_HOST_PID` stands in for the
/// process ancestry). Returns the attachment id.
fn hook_attach(project: &TestProject, agent: &str, session: &str, host: &str) -> String {
    let out = Command::new(common::rite_bin())
        .args([
            "sessions",
            "attach",
            "--agent",
            agent,
            "--harness",
            "claude",
            "--session",
            session,
            "--kind",
            "pull",
            "--ttl",
            "600",
            "--format",
            "json",
        ])
        .env("RITE_DATA_DIR", project.data_path())
        .env("RITE_HOST_PID", host)
        .current_dir(project.work_dir())
        .output()
        .expect("run rite sessions attach");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&text[text.find('{').unwrap()..]).unwrap();
    v["attachment_id"].as_str().unwrap().to_string()
}

fn live_sessions(project: &mut TestProject, agent: &str) -> Vec<serde_json::Value> {
    json(
        &project
            .agent(agent)
            .run(&["sessions", "list", "--format", "json"]),
    )["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["attached"] == true)
        .cloned()
        .collect()
}

/// A launcher hook that attaches the agent as `pull` before the channel
/// server exists is holding the identity for it. The channel, started under
/// the same Claude process, takes that attachment over and holds occupancy
/// itself; the placeholder is detached.
#[test]
fn channel_takes_over_the_hooks_pull_attachment() {
    let mut project = TestProject::with_name("channel-takeover-pull");
    let placeholder = hook_attach(&project, "claude-a", "hook-1", "4242");

    let mut server = start_with_env(&project, "claude-a", &[("RITE_HOST_PID", "4242")]);
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    // The channel is attached and the placeholder is not.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let sessions = loop {
        let live = live_sessions(&mut project, "claude-a");
        if live.len() == 1 && live[0]["kind"] == "stream" {
            break live;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "channel did not take the identity over: {live:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_ne!(sessions[0]["attachment_id"], placeholder);
    assert!(sessions[0]["session"].as_str().unwrap().starts_with("mcp:"));
    assert_eq!(sessions[0]["host_pid"], 4242);

    // Delivery works under the taken-over identity.
    let msg_id = json(&project.agent("someone").run(&[
        "send",
        "general",
        "@claude-a after the takeover",
        "--format",
        "json",
    ]))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let event = server.event_for(&msg_id);
    assert!(
        event["params"]["content"]
            .as_str()
            .unwrap()
            .contains("after the takeover")
    );

    drop(server.stdin);
    assert!(server.child.wait().unwrap().success());
}

/// The placeholder of another Claude session of the same agent, live in
/// another terminal, is that session's identity. A channel under a
/// different Claude process refuses and stops; the first session keeps its
/// attachment and its claim. So does a placeholder whose process is unknown.
#[test]
fn channel_never_takes_over_another_claude_sessions_placeholder() {
    let mut project = TestProject::with_name("channel-no-takeover-other-host");
    for (name, host) in [("claude-a", "1"), ("claude-b", "none")] {
        let other = hook_attach(&project, name, &format!("{name}-hook"), host);

        let mut server = start_with_env(&project, name, &[("RITE_HOST_PID", "4242")]);
        server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
        assert!(server.response(1)["result"].is_object());
        server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let status = server.child.wait().unwrap();
        assert!(!status.success(), "{name}: {status:?}");

        let live = live_sessions(&mut project, name);
        assert_eq!(live.len(), 1, "{name}: {live:?}");
        assert_eq!(live[0]["attachment_id"], other, "{name}");
        assert_eq!(live[0]["occupancy"], "held", "{name}");
    }
}

/// A `stream` attachment is another channel server. It is never taken over:
/// the second server refuses and the first keeps the identity.
#[test]
fn channel_never_takes_over_another_channels_stream_attachment() {
    let mut project = TestProject::with_name("channel-no-takeover-stream");
    let other = json(&project.agent("claude-a").run(&[
        "sessions",
        "attach",
        "--harness",
        "claude",
        "--session",
        "mcp:1",
        "--kind",
        "stream",
        "--ttl",
        "600",
        "--format",
        "json",
    ]))["attachment_id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut server = start(&project, "claude-a");
    server.send(serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}));
    assert!(server.response(1)["result"].is_object());
    server.send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let status = server.child.wait().unwrap();
    assert!(!status.success(), "{status:?}");

    let listed = json(
        &project
            .agent("claude-a")
            .run(&["sessions", "list", "--format", "json"]),
    );
    let live: Vec<&serde_json::Value> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["attached"] == true)
        .collect();
    assert_eq!(live.len(), 1, "{listed}");
    assert_eq!(live[0]["attachment_id"], other);
}
