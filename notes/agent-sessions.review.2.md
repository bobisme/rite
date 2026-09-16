# Agent sessions: review after phase-one testing

## Executive summary

**Proceed with a thin local bridge. Revise automatic attachment before implementation.**

The test evidence resolves the main interoperability question. Claude and Codex can exchange live Rite messages through thin adapters.
The busy-Codex test and same-directory routing test also have supporting evidence.
The document correctly leaves busy-Claude batching unverified.

The remaining design risks are identity binding and session lifecycle under a shared daemon.
These need narrow local contracts, not a six-state distributed delivery system.
The larger recovery design I previously added should remain deferred.

Reviewed source SHA-256: `1c91aa2d592226d214014e04e095a4311ecc7a0bee9ec7cb9177a30b5160ac0a`.
The proposal and application source were left unchanged. No new harnesses or messages were created.

## Evidence checked

Scratch evidence root: `/tmp/claude-1000/-home-bob-src-rite/4c85e537-b2be-4e14-aab8-b34fb5529c2e/scratchpad`.

- Codex bridges: `probe-codex/bridge.sh`, `bridge2.sh`, and `bridge*.log`.
- Claude adapter: `probe-claude/rite_channel.py` and `channel.log`.
- Hook identity: five `probe-codex/hook-*.json` dumps and `hookdump.sh`.
- Durable bus records: `rite history rite -L probe -n 40 --format json`, which returned 18 messages.
- Targeted Codex and Claude session transcripts, including the plain-Codex queue test.

Verified bus anchors include:

| Case | Request | Reply |
|---|---|---|
| Claude → Codex, busy | `01M2KFP4Q4HZSJ82SDGXH91FVE` | `01M2KFQTFQHYS2W6STMAR9JGQT` |
| Codex A → B | `01M2KGC42B7SZV0BW6WEBRYK1J` | `01M2KGC91R763PT0123VP5G7SR` |
| Codex B → A | `01M2KGEN9SS6Z9RX9R1TEET3A1` | `01M2KGEVECXDW5CDK5G61DH668` |

The busy-Codex turn ended at `21:31:56.617Z`; its queued turn began at `21:31:56.621Z`.
The reply reached Rite at `21:32:02.423Z`. This supports turn-end queuing for that tested case.
The later plain-Codex transcript completed a queued turn with `queued-ok` at `21:58:02.468Z`.

## Proposed changes

The suggested edits are independent patches against the reviewed version. They are not a complete rewrite.

## P1 — Bind identity per thread before automating attachment

**Evidence:** `notes/agent-sessions.md:152`, `:484`, and `:530`.

The hook dumps confirm empty `RITE_AGENT` values and the shared daemon as parent.
The main design still requires that environment variable or its fallback.
The appendix instead proposes putting an agent name in the workspace hook command.
That workaround cannot distinguish two differently named agents sharing the same directory, which the same appendix demonstrates.

**Change:** Keep explicit `(agent, thread_id)` binding in the launcher or bridge for the first implementation.
Use hook `session_id` to look up that binding. Do not infer identity from cwd or daemon environment.
An isolated workspace with exactly one named agent can use a static hook name as a documented restriction.
Do not claim that it supports the shared-directory case.

**Benefit / tradeoff:** This preserves the demonstrated thread routing without requiring a new protocol.
Automatic SessionStart registration remains conditional on proving how its identity binding reaches the hook.

**Acceptance:** Two named agents in one directory attach independently. Ending either session releases only its own attachment and claim.

### Suggested edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -150,7 +150,12 @@
 ### Hook integration
 
-Harness hooks read session identity from stdin. Resolve `RITE_AGENT`, with the
-existing `AGENT` compatibility fallback, and fail clearly if neither is set.
-Do not infer the routing identity from the operating-system username.
+Harness hooks read `session_id` from stdin. For daemon-hosted Codex, the
+launcher or bridge supplies an explicit `(agent, session_id)` binding; hooks
+resolve that binding instead of reading the TUI's environment. The first demo
+already used explicit agent names and thread IDs.
+
+A static agent name in a workspace hook is supported only when that workspace
+has one named agent. Shared-directory sessions need distinct per-thread
+bindings. Do not infer identity from cwd, daemon PID, or operating-system user.
 
 SessionStart attaches the exact session. Stop/PostToolUse records activity.
````

## P1 — A daemon PID cannot establish individual session liveness

**Evidence:** `notes/agent-sessions.md:127` and `:530`; five hook dump files in the probe directory.

All dumps have parent PID `1016525`. Two distinct active sessions and a placeholder share that daemon.
The hook's own PID is an ephemeral shell. Its parent is the shared daemon.
Neither identifies one harness session's lifetime.

This narrows my earlier suggestion that PID plus start time could replace a readiness protocol.
That check identifies a process correctly, but the process here hosts multiple sessions.
A daemon can remain alive after a particular session ends.

**Change:** Keep thread identity authoritative. Record SessionEnd for that exact thread.
If the bridge or daemon disappears without a session event, report unknown session state.
Do not release occupancy merely from activity silence, or keep it forever solely because the daemon survives.
A per-thread lifecycle query can improve this later, if the installed API supports it.
No new general readiness protocol is required for the thin bridge.

**Acceptance:** End one of two sessions while the daemon and the other session remain alive.
Confirm the correct attachment changes state. Also test a late placeholder SessionEnd.
The existing dumps already demonstrate why session-scoped detach is necessary.

### Suggested edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -125,5 +125,5 @@
 | Fact | Evidence | Meaning |
 |---|---|---|
-| Harness existence | Exact local session probe or normal SessionEnd | The attachment still names an existing session |
+| Harness existence | Exact thread lifecycle evidence or matching SessionEnd | The attachment still names an existing session; a shared daemon PID is insufficient |
 | Recent activity | Stop/PostToolUse hooks | The session recently performed work |
 | Delivery readiness | Adapter lease or a successful exact-session readiness probe | This delivery path is available |
````

## P2 — Promote the results into the current design and first-release scope

**Evidence:** `notes/agent-sessions.md:55`, `:215`, `:317`, `:381`, `:453`, and `:536`.

The update appends findings but leaves the original design body unchanged.
The body still says no round trip passed, requires six ledger states, and requires coordinated hook admission.
Finding 1 says plain Codex is not daemon-hosted; finding 13 explicitly retracts that conclusion.
Readers should not need to reconcile a chronological experiment log to find the supported launch recipe.

**Change:** State that interoperability is demonstrated, while the complete phase-one test matrix remains incomplete.
Move the thin adapters into the first-release plan. Treat durable replay and receipt machinery as later options.
Use ordinary occupancy claims for the deployed Edict responder convention, once live delivery works.
Renew them independently of model activity and release only the matching attachment's claim.
Keep claim-free mention hooks outside that convention unless explicitly configured.

Use the final observed daemon recipe in the setup section. Mark the earlier `--remote` diagnosis superseded.
Keep the healthy-daemon result scoped to the tested CLI/version; do not generalize it to every Codex installation.

**Benefit / tradeoff:** The design becomes smaller and reflects what was actually demonstrated.
Best-effort delivery may lose or duplicate a notification; durable channel history remains available for manual recovery.
This is an acceptable explicit first-release limit.

### Suggested edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -53,8 +53,14 @@
 and [Codex hooks reference](https://learn.chatgpt.com/docs/hooks#stop).
 
-CLI help proves command availability only. Exact Codex endpoint/profile
-selection, idle wakeup, busy handling, exit semantics, and restart behavior
-still need isolated live tests. No round-trip test has yet passed this proposal's
-acceptance gate. Do not assume identical transport semantics.
+The phase-one evidence below demonstrates cross-harness delivery, busy-Codex
+queuing, and independent routing of two Codex threads sharing a directory.
+Busy-Claude behavior and the full restart/failure matrix remain unverified.
+The complete phase-one gate is therefore still open, but interoperability is
+established on the tested versions.
+
+The first release is the thin local bridge, explicit thread identity, and
+ordinary occupancy claims for configured Edict responders. Delivery is
+best-effort. The detailed receipt ledger, replay, and coordinated admission
+sections below are deferred design options, not prerequisites for that release.
 
 ## Identity, attachments, and local state
````

## P2 — Correct timing labels and preserve direct evidence pointers

**Evidence:** `notes/agent-sessions.md:436` and `:490`; `bridge.sh`, `bridge2.sh`, `channel.log`, and both harness transcripts.

The Codex bridge records `ts` before invoking `codex queue`, then prints that timestamp after success.
Its numbers measure time until the queue attempt begins, not queue completion or model receipt.
The Claude bridge records its timestamp after writing the notification.
Those columns currently compare different boundaries.

Pong 5 was written at `21:29:54.705Z`. Claude received the channel item at `21:29:54.888Z`
and emitted its acknowledgment at `21:29:56.607Z`: about 1.9 seconds after the bus write, not within one second.
Ping 5 also needed setup intervention: Codex first printed a send command, then executed it during a later turn.
Do not use that case to claim unattended behavior. Ping 6 provides stronger evidence after setup.

The same-directory bus reply intervals are about 5.10 seconds and 6.29 seconds.
The larger table figures can include bridge handoff, but need a named measurement boundary.
The channel currently contains 18 probe messages, including the four later same-directory messages.

**Change:** Link the preserved scripts and raw records, label each clock boundary, and distinguish setup from steady-state cases.
The exact subsecond figures are not acceptance gates. Observed receipt and correct anchors are the useful result.

**Benefit / tradeoff:** Reviewers can reproduce the conclusions without turning the demo into a benchmark project.
Copy the small evidence bundle from temporary storage before relying on these paths long-term.

### Suggested edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -438,5 +438,5 @@
 | rite-dev → Claude, idle | ping 2 | push 0.18 s after write | anchored reply on the bus 4.6 s after the ping |
 | Claude → Codex, idle | ping 5 | queued 0.15 s after write | Codex turn started at once |
-| Codex → Claude, idle | pong 5 | push 0.15 s after write | Claude acknowledged within 1 s |
+| Codex → Claude, idle | pong 5 | notification write logged 0.15 s after bus write | transcript receipt at 0.18 s; acknowledgment at about 1.9 s |
 | Claude → Codex, busy (60 s sleep) | ping 6 | queued 0.13 s after write | held; handled when the sleep turn ended, reply 55 s after the ping |
 | Codex → Claude, busy | ping 7, ping 8 | push 0.05 s after write | **not a busy test**: Claude backgrounded or declined the sleep and was idle both times |
````

## Small follow-ups

The JSON stdout bug is corroborated by the Claude channel log and `src/sync/git.rs:180`.
`commit_files` inherits Git stdout via `.status()`. Fix that path before treating `rite send --format json` as reliably parseable.
It does not invalidate the demonstrated transport path.

Keep hook trust as an explicit setup decision. “Pre-seed trust” must mean installing an already-approved exact definition,
not a launcher silently approving its own newly generated commands.
The [official hook documentation](https://learn.chatgpt.com/docs/hooks#review-and-trust-hooks) requires review of non-managed definitions and binds trust to their hashes.

The claim that `--approve-for-me` removes prompts should remain a result from these runs, not an unconditional launcher guarantee.
The recorded comparison was confounded by a previously saved command approval.

## Next bounded step

Package the proven thin adapters and explicit agent/thread binding for local use.
Retain ordinary responder claims with independent renewal and session-specific release.
Before automating attach/detach, test two same-directory sessions with distinct identities and end one while the shared daemon survives.
Use a tool with an explicit ready/release handshake to test busy-Claude delivery later.
Do not add the larger ledger to resolve these local identity questions.
