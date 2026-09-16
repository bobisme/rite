# Review: Agent sessions

## Executive summary

**Verdict: REVISE before implementation.**

The core direction fits Rite: stable agent identity, separate attachments, durable messages, and harness-specific delivery adapters.
Keep the append-only source of truth and the decision to avoid session supervision.

The current plan overstates delivery guarantees. It can lose messages, expire healthy sessions, and leave responders suppressed without delivery.
Its cross-host claims exceed what asynchronous Git sync and host-local ledgers can guarantee.

The fastest useful slice is single-host Claude/Codex delivery through the existing mention stream.
`rite mentions follow` is already implemented in `src/cli/mentions.rs`.
The companion note still describes that primitive as future work.
Treat durable recovery as additional work, because the existing stream does not persist its cursor.

## Review scope and evidence

Reviewed on 2026-09-15 against Rite HEAD `17618d1ad2f9ab0e32dbf62ab2e53bf49ad257b1`.
The proposal was an untracked file. Its SHA-256 was `e2d5745c195cce62a174673c009d294823d4aac6f06e964be5f9cd58afc38c92`.

I inspected the proposal, companion note, mention classifier, hook gates, spawn path, sync path, and agent/presence types.
Installed `codex --version` returned `codex-cli 0.154.0`; `codex queue --help` confirmed the command and thread/message options.
I checked current official Claude channel and both harness hook documentation.

No live harness injection or round-trip test was performed. Queue acceptance, endpoint reachability, and restart behavior remain unverified.
No application source, proposal text, or live agent configuration was changed.

P1 findings block the guarantees stated in the proposal. P2 findings require design work before their associated feature ships.
The edits below are independent suggestions against the original file. They are not a complete rewritten specification.

## P1 — 1. Delivery accounting can lose messages permanently

**Evidence:** `notes/agent-sessions.md:135`, `:156`, `:172`, `:182`, and `:198`.

The plan consumes messages before it knows whether the harness received them.
A stream can write a notification that Claude silently drops. The ledger then prevents the Stop hook from recovering it.
A drain can mark a batch delivered, then fail before the hook prints valid JSON.
A successful process spawn can also precede a failed `codex queue` command.

This contradicts both the once-only claim and the missing-channel fallback.
Claude explicitly distinguishes transport writes from processing, and recommends application confirmation when needed.
See the [Claude channels reference](https://code.claude.com/docs/en/channels-reference#notification-format).

Rite's `spawn_hook` marks batches delivered after `Command::spawn` succeeds.
Both wait paths discard the child exit result (`src/cli/hooks.rs:1300`).
Reusing that success criterion cannot implement the proposed push-failure recovery.

**Change:** Separate reservation, transport submission, and confirmed receipt.
Serialize reservation under a cross-process lock and use an expiring attempt token.
Only receipt confirmation retires pending work when reliable delivery is promised.
An anchored reply may provide confirmation, but the ledger must actually consume that evidence.
A reply is also different from completion of the requested task.

**Benefit and cost:** Recovery becomes explicit, with possible duplicate delivery after ambiguous failures.
Exactly-once processing requires recipient idempotency. Append-only records alone cannot provide it.
Preserve an `uncertain` outcome where the transport provides no receipt signal.

**Validation:** Test competing consumers, failure after reservation, broken stdout, ignored channel notifications, and nonzero push exits.
Use deterministic barriers around handoff points.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -154,7 +154,17 @@
 ```
 
-States: `pending`, `delivered`, `failed`. Keyed on `(message_id, agent)`, so
-whichever rite command evaluates first wins and a second evaluation is a no-op,
-the same guarantee the hook trigger queue gives.
+Delivery identity is `(message_id, agent)`. Attempts also carry the attachment
+ID, an expiring reservation token, and the transport result. Reserve work under
+a cross-process lock before attempting delivery. Persist pending work first.
+
+Distinguish `pending`, `reserved`, `submitted`, `confirmed`, `failed`, and
+`uncertain`. A successful process spawn or stdout write is not confirmation.
+The push adapter must observe command exit status within a bounded timeout.
+An ambiguous result can cause retries and duplicates; recipients must dedup by
+message ID. Do not claim exactly-once delivery.
+
+An anchored reply can confirm receipt when it identifies the intended recipient
+and pending message. Otherwise use an explicit receipt operation. A transport
+without receipt evidence remains best-effort, with visible uncertainty.
 
 Consumers:
````

## P1 — 2. Hook activity does not establish idle-session liveness

**Evidence:** `notes/agent-sessions.md:35`, `:60`, `:93`, `:115`, and `:205`.

The note correctly rejects Rite-command heartbeats for idle sessions, then repeats the same defect with harness hooks.
After the final Stop hook, an idle session emits no Stop or PostToolUse heartbeat.
After 600 seconds, it expires despite remaining reachable. Long tools and approval waits can produce the same gap.
The next message can therefore spawn another agent beside a healthy session.

A Stop hook only runs when a turn ends. A message arriving after that event cannot trigger another Stop hook.
Without a working push channel, latency has no bound until another turn starts.
The [Claude hook reference](https://code.claude.com/docs/en/hooks#stop) defines that event boundary.
The [Codex hook reference](https://learn.chatgpt.com/docs/hooks#stop) confirms continuation output and `stop_hook_active`.
Those contracts do not establish an idle wakeup mechanism.

**Change:** Separate activity, existence, and delivery readiness.
Have the attached bridge renew its lease independently, or probe the exact local harness attachment when routing.
An expired activity timestamp must mean uncertain, not proven dead.
A pull-only session must display that it needs another turn.
Bound hook continuations and preserve unconsumed work when hooks are cancelled or rejected.

**Benefit and cost:** Healthy idle agents keep their messages and avoid duplicate spawns.
A bridge heartbeat or local probe adds adapter work. A larger TTL alone does not solve this problem.

**Validation:** Keep a session idle beyond TTL, then address it. Repeat during a long tool and an approval wait.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -113,6 +113,14 @@
 |---|---|---|---|
 | SessionStart | yes | yes | `rite sessions attach --harness codex --session $session_id --kind push -- codex queue --thread {session} --message {body}` |
-| Stop, PostToolUse | yes | yes | `rite sessions beat` |
-| SessionEnd | yes | yes | `rite sessions detach` |
+| Stop, PostToolUse | yes | yes | Record activity for the exact attachment; do not use it as the sole liveness signal |
+| SessionEnd | yes | yes | Detach only the exact attachment from hook stdin |
+
+A connected adapter must renew readiness independently of model turns, or supply
+a local probe for its exact attachment. Silence from activity hooks means
+uncertain, not dead. Do not cold-spawn solely because an activity TTL elapsed.
+
+A Stop hook can continue a finishing turn. It cannot wake an already-idle
+session. Pull-only delivery waits for another turn and has no latency bound.
+Respect continuation limits and leave unconfirmed messages recoverable.
 
 The channel server attaches itself as `stream` when it connects and detaches on
````

## P1 — 3. The new addressing rule omits the existing DM privacy boundary

**Evidence:** `notes/agent-sessions.md:128`.

“Every @mention” includes names mentioned inside somebody else's DM.
The proposed rule would forward that private message to a nonparticipant.
An unrestricted `--follow` can create the same problem.

Current Rite explicitly rejects this route in `MentionFilter::classify` (`src/cli/mentions.rs:120`).
It uses the channel filename for privacy decisions, excludes system messages, and matches names without case sensitivity.
The companion note also states the DM restriction (`notes/claude-code-channel-plugin.md:193`).

**Change:** Share the existing classification logic and add broadcast routing only for eligible public channels.
Preserve the companion note's trusted-local-store assumption and optional sender/label filters.
A displayed sender name is not authentication.

**Benefit and cost:** Both adapters keep the same established privacy behavior.
Extracting shared classification costs less than maintaining another subtly different router.

**Validation:** A third-party mention inside a DM must not route outside its participants.
Test malformed DM names, forged message.channel fields, self messages, case variants, and system records.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -126,8 +126,12 @@
 traffic exactly as stranded-trigger delivery does.
 
-For each message, the addressed agents are: every `@mention`, the other DM
-participant, and any agent whose session opted into the channel with
-`--follow <channel>`. For each addressed agent, resolve the live session on
-the newest `attached` or `beat` inside its TTL:
+Reuse the existing mention classifier. The channel file determines privacy.
+A DM routes only to its participants; mentions and follows cannot override that
+boundary. In public channels, route mentions and explicit channel follows.
+Exclude self-authored and system messages, normalize name comparisons, and
+apply configured sender and label filters before creating pending work.
+
+Resolve an eligible attachment only after classification. Sender names and
+synced host labels are routing metadata, not authentication.
 
 | Live session | rite does |
````

## P1 — 4. Occupancy integration neither covers all hooks nor makes phase one safe

**Evidence:** `notes/agent-sessions.md:140`, `:236`, and `:251`.

Changing `is_claim_available` does not cover every responder path.
TTL and on-exit claim gates call `stake_hook_claim` directly (`src/cli/hooks.rs:1734`).
That function checks claim records under its own atomic append (`src/cli/hooks.rs:1110`).
Mention hooks can also fire without any claim (`src/cli/hooks.rs:1786`).
The claim that every existing responder automatically stops double-spawning is incorrect.

Even successful suppression causes a phase-one regression: the session blocks spawning before any delivery path exists.
Public channel traffic adds another mismatch. A channel responder may be suppressed even when its session did not opt into that channel.
No agent then receives the trigger.

**Change:** Enable suppression only after durable acceptance for the same recipient and message.
Define responder identity explicitly for applicable hooks, including mention and claim-taking paths.
Leave unrelated hooks alone. Integrate the decision with hook queue admission so one message does not enter both paths.
Keep session-only phase one observational.

**Benefit and cost:** A session record cannot create a message black hole.
This requires an actual admission design and may require hook metadata changes.

**Validation:** Cover claim-free mention hooks, TTL claims, on-exit claims, existing spawn queues, and unsubscribed channel traffic.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -138,7 +138,12 @@
 | none | fall through to hooks, which spawn as today |
 
-A live session for agent X makes `claim_available` on `agent://X` return false.
-Every existing responder hook then stops double-spawning with no edits to the
-hook or to edict.
+Session visibility alone does not suppress hooks. Suppress a responder only
+when a ready attachment has durably accepted this message for that recipient.
+Apply admission consistently to claim-free mention hooks, atomic claim-taking
+hooks, and queued triggers. Define how each responder identifies its recipient.
+Unrelated hooks retain their existing behavior.
+
+Ship the session-only phase as observation. Enable spawn suppression together
+with the corresponding working delivery path and recovery behavior.
 
 Codex queues internally and channels queue at turn boundaries, so `push` and
````

## P1 — 5. Cross-host ownership cannot be inferred from the newest heartbeat

**Evidence:** `notes/agent-sessions.md:64`, `:131`, `:150`, and `:201`.

During delayed sync, two hosts can each believe their local attachment is newest.
Each has an independent ledger, so both can deliver the same message.
Later convergence cannot undo those deliveries. Normal heartbeats can also switch the winner repeatedly.
Wall clocks do not provide exclusive ownership.

The local lifecycle has a related gap: `beat` and `detach` need an attachment identity, not just the agent name.
An old session's late detach must not retire its replacement.
A channel server disconnect must not erase a still-valid pull fallback.
The document says hooks read session IDs, but does not define how every state transition is fenced by that identity.

**Change:** Use an immutable attachment ID and deterministic event folding.
Reject duplicate active identity ownership locally, or require explicit replacement.
Make heartbeat and detach conditional on the exact attachment.
Defer automatic cross-host failover until ownership and partition semantics are specified.

Store executable adapter configuration locally. Sync descriptive session metadata only if needed.
A copied `host` string must not authorize execution of a command from synced JSONL.
Document the same-user trust boundary before broader routing.

**Benefit and cost:** A single-host first version is tractable and avoids false exclusivity guarantees.
Cross-host convenience remains deferred. Multi-host availability and exclusive ownership require an explicit tradeoff.

**Validation:** Test late detach, duplicate registration, reconnect, event reordering, clock skew, and disconnected hosts.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -199,8 +199,14 @@
   records `failed`, writes `detached`, and the message falls through to hooks
   on the next evaluation. Self-healing, no operator step.
-- **Two hosts, one agent name.** Both see the live session via sync. Only the
-  session's host executes. Neither spawns. If the agent is attached on both
-  hosts, the newest record wins and the other is stale; `rite doctor` should
-  report it.
+- **Attachment replacement.** Each attach creates an immutable attachment ID.
+  Beats and detach events apply only to that attachment. A late event from an
+  old session cannot replace or retire the current attachment. Bridge readiness
+  and harness existence are separate state.
+- **Two hosts, one agent name.** First release supports local ownership only.
+  Synced visibility does not grant execution authority or exclusive ownership.
+  Cross-host routing requires a separate partition, replay, and ownership
+  contract. Do not elect an owner by the newest heartbeat.
+- **Adapter trust.** Keep executable adapter configuration local. A synced host
+  label cannot authorize a command. Document the trusted same-user boundary.
 - **Channel flag missing.** Stream never delivers; Stop hook drains instead.
   Latency becomes one turn instead of immediate.
````

## P2 — 6. Send-time routing lacks a replay path for imported or interrupted work

**Evidence:** `notes/agent-sessions.md:124`, `:136`, `:198`, and `:257`.

`rite sync pull` rebuilds the index but never evaluates hooks (`src/cli/sync.rs:64`).
The stranded sweep consumes existing hook-queue records (`src/cli/hooks.rs:1448`).
It does not rediscover arbitrary imported messages or failed delivery records.
A later command therefore cannot provide recovery without new reconciliation logic.

Hook evaluation also returns early when no hooks exist (`src/cli/hooks.rs:1896`).
Send skips it for `--no-hooks` and `!nohooks` (`src/cli/send.rs:270`).
Placing routing inside that pass would inherit these conditions unless explicitly separated.
The plan needs to decide whether those flags suppress spawning only or all delivery.

The “host-local” ledger also needs real storage exclusion.
Sync stages all files (`src/sync/git.rs:210`), and current ignores do not exclude `deliveries.jsonl`.
Existing stores need migration, not just changed initialization defaults.

**Change:** Specify routing triggers, durable discovery progress, replay identity, and startup history policy.
Recover the append-to-channel / append-to-ledger crash gap from channel messages.
Define bounded batches, retry limits, and visible exhaustion.
For the first local version, use one source reader shared with the existing mention stream.

**Benefit and cost:** Recovery has a concrete execution path.
Durable replay needs more than the existing stream's in-memory offsets, which start existing channels at EOF.
See `src/cli/mentions.rs:183`.

**Validation:** Test append-before-crash, no configured hooks, disabled hooks, sync import, restart, and ledger exclusion from Git.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -255,5 +255,11 @@
   follow is opt-in per session so a broadcast channel does not wake every
   agent on every message.
-- Does `rite sync pull` evaluate hooks today? If not, a push session on the
-  other host waits for that host's next rite command, which is the existing
-  stranded-trigger behavior and acceptable.
+- `rite sync pull` currently rebuilds indexes but does not evaluate hooks.
+  The hook sweep only consumes its own queued triggers. Define a separate
+  delivery reconciliation path for imported messages and interrupted sends.
+- Specify which commands invoke reconciliation, startup replay boundaries,
+  durable discovery progress, bounded batches, and retry exhaustion.
+- Decide whether `--no-hooks` suppresses live delivery. Routing must work with
+  no configured hooks and must not depend on the hook evaluator's early returns.
+- Keep local delivery state outside synced files, or migrate Git exclusions for
+  both existing and new stores. Verify it remains absent from staged changes.
````

## P2 — 7. The Codex command drops the reply context promised by the plan

**Evidence:** `notes/agent-sessions.md:79`, `:114`, and `:189`.

The example delivers only `{body}`. It does not carry the Rite message ID, source channel, sender, or reply target.
The later claim that every delivery carries an anchor therefore does not hold for the shown Codex adapter.
Having optional placeholders does not ensure adapters use them.

**Change:** Define a common message envelope and a required plain-text rendering for prompt-based adapters.
Include provenance, message ID, channel, sender, route, and reply target.
Represent message content as peer input. Do not describe it as an authoritative system instruction.
Define attachment references and size limits rather than flattening everything into an unbounded argument.

**Benefit and cost:** Replies can correlate across both harnesses, and adapter behavior becomes testable.
The prompt grows slightly. Per-message anchors must survive batching.

**Validation:** Send Claude-to-Codex and Codex-to-Claude messages through isolated sessions.
Check actual transcript receipt and anchored replies, including a batch with two independent roots.
Check the selected Codex endpoint/profile and exact thread ID. CLI help alone does not prove reachability.

### Proposed plan edit

````diff
--- a/notes/agent-sessions.md
+++ b/notes/agent-sessions.md
@@ -77,5 +77,5 @@
   "session": "3f2a9c1e-...",
   "kind": "push",
-  "deliver": ["codex", "queue", "--thread", "{session}", "--message", "{body}"],
+  "deliver": ["codex", "queue", "--thread", "{session}", "--message", "{rendered}"],
   "ttl_secs": 600,
   "event": "attached"
````

## Recommended sequence

1. Prove one Claude-to-Codex-to-Claude round trip on the same host, using exact session IDs and anchored replies.
2. Define attachment identity, readiness, message envelopes, and honest receipt semantics.
3. Implement adapters over shared mention routing, with bounded retries and durable recovery where supported.
4. Enable responder suppression only when message handoff works through every applicable hook gate.
5. Design cross-host ownership and replay separately, after the local path passes failure tests.

### Acceptance boundary

A local demo proves interoperability. It does not prove exactly-once delivery, crash recovery, or cross-host exclusivity.
Report transport submission, observed receipt, and task acknowledgment separately.

The first implementation should also test cold restart, an idle session beyond TTL, missing channel enablement, and session replacement.
Keep the original proposal unchanged until these decisions are incorporated.
