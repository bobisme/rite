# Agent sessions: routing messages to live harnesses

Status: shipped in rite 0.35.0 on 2026-09-17. The session registry is
`rite sessions` (bn-3c4d, 7aa978b). Delivery into Codex is push-at-send:
`rite send` itself invokes the host-configured adapter for every live push
attachment the message addresses (bn-36x1, fa557ee). Delivery into Claude is
`rite channel`, a stdio MCP channel server Claude Code spawns, which attaches
itself and replies through a tool (bn-1yya, 18d843c, security review
cr-od7adl). There is no per-session bridge process. The first release is
therefore push-at-send, the channel server, explicit thread identity, and
occupancy claims owned by their attachment. Delivery is best-effort and
occupancy is advisory, like every rite claim. The receipt ledger, durable
replay, and coordinated hook admission remain deferred design options, kept in
[Deferred design options](#deferred-design-options), not prerequisites.

Companion to [claude-code-channel-plugin.md](claude-code-channel-plugin.md),
which describes the Claude channel server. This note supersedes that note's
per-channel followers: `rite mentions follow` exists and is the integration
primitive. Reviews: [agent-sessions.review.1.md](agent-sessions.review.1.md),
[agent-sessions.review.2.md](agent-sessions.review.2.md).

## Goal and first release

Let a Claude agent and a Codex agent exchange rite messages through their live
sessions, without asking either model to poll its inbox.

One host, one trusted same-user rite data directory. Reuse rite's messages,
mention classifier, and reply anchors. Keep agent identity separate from a
harness attachment. Keep adapter configuration local. Do not promise
exactly-once delivery or cross-host ownership.

rite remains a coordination primitive. It records reachability and invokes
bounded delivery adapters. It does not supervise, restart, or schedule agents.
Existing hooks remain responsible for cold spawning.

## What is established

Checked on 2026-09-15 with `codex-cli 0.154.0` (`gpt-6-astra`) and Claude Code
v2.1.273. Results are scoped to those versions.

| Need | Claude Code | Codex |
|---|---|---|
| Submit text to a live session | Enabled channel server sends `notifications/claude/channel` | `codex queue --thread <id> --message` to a daemon-hosted thread |
| Continue a finishing turn | Stop hook returns `{"decision":"block","reason":...}` | Same continuation shape |
| Hook session identity | `session_id`, `cwd` on hook stdin | `session_id`, `cwd`, `transcript_path` on hook stdin |
| Normal session exit | SessionEnd hook | SessionEnd hook |
| Cold spawn | Existing `claude -p` hook | Existing `codex exec` hook |

Demonstrated: cross-harness delivery in both directions, busy-Codex queuing,
busy-Claude batching, and independent routing of two Codex threads sharing a
directory. Unverified: the restart and failure matrix. The complete
phase-one gate is still open; interoperability is established on the tested
versions.

Documented, not observed: Claude's notification await confirms a transport
write, not receipt; disabled or policy-blocked channels discard silently; a
busy Claude session batches events for its next turn
([channels reference](https://code.claude.com/docs/en/channels-reference#notification-format)).
Stop hooks run at a turn boundary and cannot wake an idle session
([Claude](https://code.claude.com/docs/en/hooks#stop),
[Codex](https://learn.chatgpt.com/docs/hooks#stop)).

### Observed results

Measurement boundaries. "Bus write" is the message `ts` in `#rite`. "Queue
call" is the bridge timestamp taken immediately before invoking `codex queue`,
so it measures time to the attempt, not completion. "Notification write" is the
Claude adapter's log line after writing to the MCP transport. "Receipt" is the
channel item in the Claude transcript. Subsecond figures are descriptive, not
acceptance gates; the useful result is observed receipt with correct anchors.

| Case | Request | Bus write → adapter | Handled |
|---|---|---|---|
| rite-dev → Claude, idle | ping 2 | notification write 0.18 s | anchored reply on the bus 4.6 s after the ping |
| Claude → Codex, idle | ping 5 | queue call 0.15 s | Codex turn started at once. **Setup case**: Codex printed the reply command instead of running it and needed two corrections, so this is not evidence of unattended behavior |
| Codex → Claude, idle | pong 5 | notification write 0.15 s | transcript receipt 0.18 s; acknowledgment 1.9 s after bus write |
| Claude → Codex, busy (60 s sleep) | ping 6 | queue call 0.13 s | held. Sleep turn ended 21:31:56.617Z, queued turn began 21:31:56.621Z, anchored reply on the bus 21:32:02.423Z, 55 s after the ping |
| Codex → Claude, busy | ping 7, ping 8 | notification write 0.05 s | **not a busy test**: Claude backgrounded or declined the sleep and was idle both times |
| rite-dev → Claude, busy (mid-generation), 2026-09-16 | ping busy | notification write 0.17 s | **held to the turn boundary**: pushed at 19:48:28 during a 7 s generation turn, inbound line rendered when that turn ended, handled in the next turn, anchored reply on the bus 8.0 s after the ping |
| Codex A → B → A, one directory | ping a1 | queue call into B 19 ms | anchored reply queued into A; bus round trip 5.10 s |
| Codex B → A → B, one directory | ping b1 | queue call into A 200 ms | anchored reply queued into B; bus round trip 6.29 s |
| plain `codex` TUI, daemon up | `codex queue` by hand | | queued turn completed with the requested reply |

Every reply carried `reply_to` pointing at its request, so
`rite wait --reply-to` was the acknowledgment with no extra machinery. Both
harnesses hold a message that arrives mid-turn until the running turn ends,
then take it as the next turn; neither steers mid-turn. The Claude case was
produced with a long generation rather than a sleep, since the harness
backgrounds long shell commands.
Two sessions sharing a working directory on the daemon were addressed
independently and neither saw the other's queued text.

Durable anchors on `#rite`, label `probe`, 18 messages in total:

| Case | Request | Reply |
|---|---|---|
| Claude → Codex, busy | `01M2KFP4Q4HZSJ82SDGXH91FVE` | `01M2KFQTFQHYS2W6STMAR9JGQT` |
| Codex A → B | `01M2KGC42B7SZV0BW6WEBRYK1J` | `01M2KGC91R763PT0123VP5G7SR` |
| Codex B → A | `01M2KGEN9SS6Z9RX9R1TEET3A1` | `01M2KGEVECXDW5CDK5G61DH668` |

Scripts and the reconstruction caveat are in
[evidence/agent-sessions/](evidence/agent-sessions/README.md).

## Launch recipe

The recipe that worked, and that a launcher such as edict would encode.

### Codex

1. Ensure the app-server daemon is running: `codex app-server daemon start`,
   or `daemon bootstrap` for a durable install. The daemon did not survive a
   host reboot here, and a stale pid file made it look alive.
2. Launch a plain `codex` in the workspace. With the daemon up, the TUI's
   thread is daemon-hosted and `codex queue` reaches it. `--remote unix://` is
   not required; if used, pass `-C <dir>` or the thread takes the daemon's
   working directory.
3. Put the agent name in the instructions and in every `--agent` flag. The
   agent's shell and its hooks run in the daemon's environment; variables set
   on the TUI process do not reach them.
4. Approval: `--approve-for-me` produced no prompt for `rite send` in one run.
   Treat that as a result from these runs, not a guarantee; the comparison was
   confounded by a previously saved "don't ask again" approval.
5. No bridge. Every `rite send` that mentions or DMs the agent runs the
   `codex` adapter for the attachment's exact thread (`codex queue --thread
   <session_id> --message <rendered>`), from the sender's host. A push that
   finds the thread gone reports `session_gone`; it never detaches.
6. Occupancy, in this order: `rite sessions reserve --harness codex` **before**
   starting the harness, then start it, then
   `rite sessions attach --attachment <id> --session <session_id>` once the id
   is known. The reservation already holds `agent://<name>`, so no responder can
   start in the gap. A direct `attach` after the harness is running cannot
   protect it from a responder that won the identity first; it refuses and
   says so. The launcher calls `rite sessions renew --attachment <id>` before
   the claim's TTL or lets it lapse (default eight hours), and a SessionEnd
   hook in the workspace's `.codex/hooks.json` runs
   `rite sessions detach --session $session_id`. None of this is built in
   edict yet; see [Loose ends](#loose-ends).

The session id is created at the first prompt, not at TUI launch. It appears
as the rollout filename under `~/.codex/sessions/` and in `session_index.jsonl`,
and on SessionStart hook stdin.

### Claude

1. Register the channel server in the project `.mcp.json`:
   `{"mcpServers": {"rite": {"command": "rite", "args": ["channel", "--agent", "<name>"]}}}`.
   A server loaded via `--mcp-config` is not found by `server:<name>`.
2. Launch with `--dangerously-load-development-channels server:rite`. The
   first launch shows a consent dialog for the new MCP server and a
   development-channels warning. Both are TUI prompts the launcher answers.
3. `rite channel` is the adapter: a stdio MCP server declaring
   `capabilities.experimental["claude/channel"]`, consuming the agent's
   mention stream, emitting one notification per record with `from_agent`,
   `channel_name`, `reply_target`, `route`, and `msg_id` as meta, and exposing
   a `reply` tool. After `notifications/initialized` it attaches itself
   (harness `claude`, kind `stream`, session `mcp:<pid>`), renews the claim
   while it runs, delivers and replies only while its attachment still owns
   `agent://<name>`, and detaches when Claude closes its stdin. It refuses to
   serve when the identity is held elsewhere, and stops when a replacement
   takes it.
4. The launcher does not attach for Claude. edict's Claude hooks (edict
   bn-3oml) attach by the hook payload's `session_id` as kind `pull`, which
   the channel must take over; until rite bn-316s ships that takeover, the
   channel refuses an identity those hooks already hold, so a Claude session
   with both the edict hooks and the channel gets the hooks' occupancy and no
   channel delivery.

Channels are a research preview and the flag syntax may change.

## Identity and attachments

`Agent` remains the append-only registration event in `src/core/agent.rs`.
Presence remains activity derived from rite commands. Claims remain advisory
locks. None of these alone says whether a harness can receive a message.

An attachment binds one agent to one exact harness session id. Each attachment
has an immutable id. Reconnecting an adapter does not replace the harness
identity.

### Binding identity to a thread

Hooks cannot supply the agent name. Every hook dump had the shared daemon as
parent and empty environment variables, and a static name in a per-workspace
hook cannot distinguish two agents in one directory, which is a case this note
demonstrates. Do not infer identity from cwd, daemon pid, or operating-system
user.

The first implementation binds `(agent, session_id)` in the launcher:

1. The launcher knows the agent name. It starts the harness and sends the
   first prompt carrying a unique nonce.
2. It finds the rollout whose transcript contains that nonce, reads the
   session id, and runs `rite sessions attach --agent <name> --session <id>`.
3. Bridges are started per session id, so they are bound by construction.

Detach needs no identity. SessionEnd stdin carries `session_id`, so one
generic workspace hook runs `rite sessions detach --session $session_id`.
An id that was never attached, such as the launch-time placeholder thread, is
a no-op.

A static agent name in a workspace hook is supported only when that workspace
has exactly one named agent, as a documented restriction. Shared-directory
sessions need the launcher binding above.

### Attachment record

Local, append-only, not synced:

```json
{
  "seq": 42,
  "ts": "2026-09-15T18:02:11Z",
  "attachment_id": "01...",
  "agent": "rite-dev",
  "harness": "codex",
  "session": "01a0a711-...",
  "kind": "push",
  "event": "attached"
}
```

`seq` is a local sequence allocated under the registry lock; fold by it, not by
wall clock. Preserve unknown fields and event variants through `core::wire`.
Only one attachment owns an agent identity locally; a second attachment needs
`--replace <attachment-id>`. Detach names the session it affects. A late event
from an old session cannot retire or replace its successor. This is not
hypothetical: the placeholder thread's SessionEnd arrived about a minute after
the real thread's SessionStart.

### Liveness

Track three facts separately:

| Fact | Evidence | Meaning |
|---|---|---|
| Harness existence | Exact thread lifecycle evidence or matching SessionEnd | The attachment still names an existing session; a shared daemon pid is insufficient |
| Recent activity | Stop and PostToolUse hooks | The session recently performed work |
| Delivery readiness | Codex: the app-server daemon answers `codex queue` for that thread. Claude: the channel process is alive and holds the attachment | This delivery path is available |

A daemon pid identifies the daemon, which hosts many threads and outlives any
one of them. Thread identity is authoritative. Record SessionEnd for the exact
thread. A failed `codex queue` on that thread is evidence the thread is gone.
If the channel or daemon disappears without a session event, report the session
state as unknown. Do not release occupancy from activity silence alone, and do
not keep it forever because the daemon survives. Never cold-spawn solely
because an activity timestamp expired; long tools and approval waits look the
same.

Transport kinds:

- `push`: `rite send` invokes the host-configured adapter, a bounded command
  for the exact session. Codex.
- `stream`: a connected adapter writes harness notifications. The Claude
  channel server.
- `pull`: a finishing-turn hook takes pending work. An idle session waits for
  another turn.

## Occupancy claims and responder hooks

The deployed edict responders gate on `agent://<name>` via `claim_available`.
Ordinary claims are therefore the integration path:

- `sessions attach` stakes `agent://<name>` for the attachment.
- Renewal depends on the attachment's kind. A `stream` attachment is renewed
  by `rite channel` while it runs, so an idle Claude session stays occupied
  and a dead channel lets it lapse. A Codex `push` attachment is renewed by
  its launcher or lapses at its TTL. A `pull` attachment made by edict's
  Claude hooks carries a ten-minute claim that tool activity renews, so an
  idle pull session lapses; nothing can be delivered to it anyway.
- `sessions detach` releases only the matching attachment's claim.
- Enable this once live delivery works for that session, not on registration
  alone.

Observed: the first probe ping woke `edict:rite:responder` for `rite-dev`
while rite-dev was live in a terminal; the spawn failed and posted noise on the
channel. After `rite claims stake agent://rite-dev` by hand, the next ping did
not spawn. The responder exit code 4 itself is a separate edict problem.

Claim-free mention hooks exist and are outside this convention unless
explicitly configured. Session registration never suppresses them.

## Routing and message envelope

Reuse `MentionFilter` in `src/cli/mentions.rs`:

1. The channel filename is the authority for DM participation.
2. DMs route only to participants. Mentions and follows cannot override that.
3. Public-channel mentions and explicit public-channel follows route;
   agent-name comparison is case-insensitive.
4. Self-authored messages and system bookkeeping are excluded.
5. Sender and label filters apply. Sender names are not authenticated.

Broadcast subscription to a channel is opt-in per session. Session existence
does not subscribe an agent to every channel its responder serves.

Every adapter renders the same fields: message id, channel, sender, route,
reply target, body, and attachment references. Present bodies as peer
messages, not privileged instructions. Each message keeps its anchor when
batched. Substitute placeholders once as argument values, never through a
shell. Set payload and batch-byte limits; an oversized message stays pending
with a visible reason rather than being truncated past its anchor.

## Delivery contract: best-effort

Three states, keyed on `(message_id, agent)`:

| State | Meaning |
|---|---|
| `pending` | Addressed message not yet handed to an adapter |
| `submitted` | Adapter reported transport acceptance. Not receipt, not task completion |
| `failed` | Adapter definitively did not submit |

A ULID enables the recipient to deduplicate; it does not prevent loss. A
notification may be lost or duplicated. The channel history is durable, so
manual recovery is always possible with `rite history` and `rite inbox`.
`rite wait --reply-to <id> --from <agent>` remains the conversation
acknowledgment. This is the explicit first-release limit.

`spawn_hook` marks a batch delivered when the process starts and ignores exit
status. Push adapters need a different completion contract: record exit status
and bounded diagnostics, enforce a timeout, and treat adapter errors by their
verified meaning.

## Local state

```text
local/
  sessions.jsonl   # attachment events
```

Authoritative JSONL; SQLite views derived and rebuildable. `sync push` stages
with `git add -A` (`src/sync/git.rs`), so `local/` needs an ignore rule in
`sync init` and a `doctor` check. Existing stores need the rule added and any
already-tracked file removed from the index; a `.gitignore` entry alone does
not untrack it. Attachments do not sync. A later design may sync descriptive
session metadata; it must never turn a synced hostname or command into local
execution authority.

## Failure cases

| Case | Required behavior |
|---|---|
| Healthy session idle beyond activity TTL | Activity is not liveness; the channel's or launcher's renewal keeps occupancy; a pull attachment lapses by design |
| Session crashes without SessionEnd | The channel dies with it and detaches; a Codex push attachment lapses at its TTL; queue failure confirms; no speculative spawn on silence alone |
| Stream disconnects | The channel stops and detaches; the messages stay on the bus for `rite inbox` |
| Adapter fails after possible submission | Record `failed` or `submitted` honestly; no automatic retry in the first release |
| Old session emits a late SessionEnd | Detach matches on session id; a never-attached id is a no-op |
| Channel disabled or blocked by policy | Notifications discard silently; report channel readiness and keep the Stop-hook pull fallback |
| Two hosts use the same agent name | No exclusive-owner guarantee; cross-host ownership unsupported |
| Daemon down or stale pid file | `codex queue` fails on every thread; the launcher checks `daemon version` before spawning |

## CLI

Shipped in bn-3c4d:

```text
rite sessions reserve --harness <h> [--kind push|stream|pull] [--ttl 8h] [--window 10m]
rite sessions attach  --harness <h> --session <id> [--kind] [--ttl] [--replace <attachment-id>]
rite sessions attach  --attachment <id> --session <id> [--ttl]      # bind a reservation
rite sessions detach  --session <id> | --attachment <id>            # no identity needed
rite sessions renew   --attachment <id> [--ttl]                      # from the channel or the launcher
rite sessions list    [--name <agent>] [--all]
```

Shipped in 0.35.0:

```text
rite send <target> <message> [--adapter <name>]   # pushes into live push attachments itself
rite channel --agent <name> [--renew 5m] [--no-attach] [-L label]   # stdio MCP channel server
```

Records live in `local/sessions.jsonl`, ignored by `sync init` and dropped
from the index by `sync commit`. `rite agents` shows the live attachment.
Adapters are named in `local/adapters.json`; `codex` is built in. There is
no external bridge script. `src/core/session.rs`, `src/cli/sessions.rs`,
`src/cli/send.rs` (push), `src/cli/channel.rs`.

### What the implementation guarantees

Nine rounds of dedicated security review (Seal cr-3e1qze) shaped this. Every
finding but the last was fixed:

- Uniqueness per agent and per session id is decided under the sessions-file
  lock; a losing concurrent attach writes nothing.
- The `agent://` claim carries its owning attachment id. Release and
  extension are compare-and-appends that check that owner under the claims
  lock, so a stale detach or renew from a replaced attachment cannot touch a
  successor's claim. The generic `claims release` and `claims refresh` skip
  owned claims. Ownerless occupancy is refused, never adopted.
- Attach reserves, occupies, then commits; the commit holds the claims lock
  across the session append and re-verifies before releasing it, so no
  `attached` record survives lost occupancy. Bind renews under the claims
  lock as its validation.
- Replacement hands the claim to the successor without a gap, and a
  replacement that fails or times out after the claim moved hands it back to
  the live predecessor rather than releasing it.
- Every session command reconciles crash leftovers for its agent: abandoned
  reservations are retired, orphaned owned claims are released or handed
  back. Session and claim reads fail closed on unreadable records, judged on
  the same locked snapshot as the append. An unknown lifecycle event keeps
  the identity reserved.
- Responder hooks read the reservation inside their locked claim stake, so a
  hook and an attach cannot both proceed. Every occupancy transition enters
  the sync auto-commit path.
- The synced claim carries no harness session id.

### What it does not guarantee

Occupancy is advisory, as every rite claim is. `rite sync pull` merges with
git and can replace `claims.jsonl` under a held file lock, so a fenced
commit can validate a snapshot a concurrent pull has superseded. This is a
property of all of rite's locks, not of sessions, and the owner accepted it
for phase one rather than serialise sync with storage writes (the review
was closed with that decision recorded). A data-directory-wide lock shared
by storage writes and `sync pull` would close the class for everything;
removing sync altogether, which nobody uses, would too.

A direct `attach` cannot protect a harness that was started before any claim
existed. Launchers that must never overlap a responder use `reserve` first.

## Phasing and acceptance

1. **Local interoperability.** Demonstrated for the cases above. Still open:
   busy-Claude delivery, which needs a tool with an explicit ready/release
   handshake since the harness backgrounds long commands; endpoint restart;
   missing channel enablement.
2. **Attachment contracts.** Shipped (bn-3c4d). Two named agents in one
   directory attach independently; ending one while the daemon and the other
   survive changes only its attachment and releases only its claim; a late
   placeholder SessionEnd is a no-op. All covered by `tests/sessions.rs`.
3. **Occupancy enabled for configured edict responders** once a session's
   delivery has worked. Claim-free hooks untouched.
4. **Deferred options** below, only if best-effort proves insufficient.
5. **Cross-host routing**, designed separately.

## Deferred design options

Kept from review 1 as options, not requirements. None is needed for
push-at-send or the channel server.

- **Receipt ledger.** Reservation tokens, `reserved` and `confirmed` and
  `uncertain` states, an explicit receipt tool, reconciliation of anchored
  replies into the ledger. Needed only for automatic retry, which also needs
  recipient-side deduplication.
- **Durable discovery.** A recipient-scoped checkpoint with an explicit
  activation boundary, reconciliation by message id after git rewrites,
  bounded backlog. `rite mentions follow` seeds at EOF and keeps offsets in
  memory, which is fine for a live session and not a replay guarantee.
  `rite sync pull` does not evaluate hooks and the stranded-trigger sweep
  reads only the hook queue, so imported messages would need explicit
  integration.
- **Stop-hook drain.** A `drain` that reserves a batch and renders
  continuation JSON. Useful as a pull fallback for a session launched without
  the channel flag; still cannot wake an idle session.
- **Coordinated admission.** One protocol for the delivery path and hook queue
  so a message never enters both. Required only if suppression is extended to
  claim-free hooks or to sessions whose delivery is unverified.

## Experiment log

Numbered findings as recorded during the tests, kept for traceability.

1. **Superseded by 13.** `codex queue` failed with `direct app-server input is
   not allowed for unloaded spawned sub-agents` and `--remote unix://` fixed
   it. The actual cause was a daemon that was not running behind a stale pid
   file.
2. `--remote` takes the daemon's working directory unless `-C <dir>` is given.
3. The session id is the rollout filename or `session_index.jsonl`, and the
   SessionStart hook's `session_id`.
4. Codex prompted for `rite send`; "don't ask again for `rite send`" persisted
   across sessions in that project.
5. `server:<name>` channel lookup ignores `--mcp-config`; `.mcp.json` works,
   with a consent dialog and a development-channels warning.
6. rite bug: `rite send --format json` stdout carries git auto-commit output
   before the JSON. `commit_files` in `src/sync/git.rs` runs git with
   `.status()`, inheriting stdout. Tracked as bn-3lbh.
7. The occupancy claim suppressed the responder double-spawn.
8. Both adapters were thin consumers of `rite mentions follow`.
9. `RITE_AGENT` set on the TUI was empty in the agent's shell under `--remote`;
   `pwd` showed the `-C` directory.
10. `--approve-for-me` produced no prompt; confounded by finding 4.
11. SessionStart fires for daemon-hosted threads at the first prompt with
    `source: startup`. The launch-time placeholder thread's SessionEnd
    (`reason: other`) arrived about a minute after the real thread's
    SessionStart.
12. Hooks run as children of the daemon with its environment. The
    per-workspace static-name workaround recorded here is withdrawn for
    shared directories; see [Binding identity to a thread](#binding-identity-to-a-thread).
13. A plain `codex` TUI is daemon-hosted when the daemon is running; its hook
    ran under the daemon and `codex queue` into its thread was answered.
14. Project hooks need one-time trust via the startup dialog or `/hooks`,
    recorded by hash under `hooks.state` in `~/.codex/config.toml`. Trust
    means installing an already-reviewed exact definition, never a launcher
    approving its own generated commands. SessionEnd hook timeouts are
    clamped to 3 s.

## Loose ends

- edict bn-3oml (review cr-2bkg32): the global Claude hooks attach by the hook
  payload's `session_id` (kind `pull`, ten-minute claim renewed by tool
  activity, detached at SessionEnd) instead of staking an ownerless claim,
  which `rite channel` refused. The per-session bridge is gone from the
  design. Still not built in edict: the Codex launcher side (`reserve` →
  start → `attach --attachment`, a `.codex/hooks.json` SessionEnd detach);
  edict installs no Codex hooks.
- rite bn-316s: `rite channel` must take over the same agent's `pull`
  attachment made by those hooks, or the two cannot share a session.
- rite bn-v0qq: `rite sessions attach|detach -q` prints a Debug dump.
- The security review of `rite channel` (cr-od7adl) ran twenty-three rounds;
  every fix is listed under 0.35.0 Fixed in CHANGELOG.md. The reviewer's
  scope grew to the mention cursor, `sessions --replace`, sync path quoting,
  and reply lifecycle, all of which shipped.
- `notes/agent-sessions.review.*.md` and Seal cr-3e1qze hold the review
  history; the last finding is recorded above as an accepted limitation.
- Sync is unused in practice; removing it would also remove the advisory
  window above.
- Nineteen `-L probe` messages remain on `#rite`.
