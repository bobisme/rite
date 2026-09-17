# Changelog

All notable changes to this project are documented here. This project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Push-at-send: `rite send` delivers into live sessions itself.** After
  writing a message, `send` folds `local/sessions.jsonl` and, for every live
  push-kind session whose agent the message addresses (an `@mention` or a
  DM, never the sender's own session), runs the harness's push adapter from
  the sender's process: `codex queue --thread <session> --message <envelope>`
  for Codex, or a named adapter from the sending host's
  `local/adapters.json`, chosen with `--adapter` on `sessions attach` or
  `reserve`. A session record names an adapter; it never carries a command,
  so a recipient cannot choose what a sender's process executes. Nothing
  runs between rite commands, the same way hooks work. A push result never
  evicts a session: no adapter result can prove that every process it
  started has finished, so a push only reports, including the adapter's own
  claim that the session is gone (`session_gone`: the Codex adapter
  reporting the thread does not exist, or any adapter exiting 66).
  Occupancy ends through the harness's SessionEnd hook, an explicit
  `sessions detach`, or a lapsed reservation. Adapters run in their own
  process group with a minimal environment (PATH, HOME, USER, LANG, TERM,
  and the RITE_* fields), in the data directory's `local/`, with a bounded
  run time and stderr drain, and the group is killed on every outcome as
  hygiene; bodies over 64 KiB stay on the bus. Only the newest attachment
  of an agent is pushed, and a superseded predecessor is retired on the
  next session command. Owned occupancy claims are written under a
  `session:<attachment>` principal so an older rite's generic release or
  refresh cannot touch them, and `owner` is sticky across a claim's
  records. Delivery is refused while `local/` is tracked by sync; `sync
  push` inspects the tree of every commit it would send on `main` and
  refuses if any contains `local/**`; `sync pull` resolves what it fetched
  to one commit, refuses it if it tracks `local/**`, and merges that exact
  commit; `rite doctor` reports tracked host-local state. Every guard
  matches the `local` path component case-insensitively, since a
  case-insensitive filesystem aliases `LOCAL/` to it. `--no-hooks` and
  `!nohooks` suppress delivery as they suppress hooks. The JSON envelope
  from `send` reports `deliveries`.

- **`rite sessions attach|detach|renew|list`: record which live harness
  session an agent is reachable in.** An attachment binds an agent to one
  exact Claude Code or Codex session id and stakes the ordinary
  `agent://<name>` claim, which is what responder hooks already gate on, so
  an agent that is live in a terminal is no longer spawned a second time by
  its own responder. `detach` is keyed on the session id alone, because a
  SessionEnd hook knows nothing else, and a session that was never attached
  is a no-op: Codex emits a late SessionEnd for its launch-time placeholder
  thread a minute after the real thread starts, and that must not retire
  the real one. `renew` is for the bridge that owns the session, not for
  activity hooks, so an idle session stays occupied and a dead bridge lets
  the claim lapse. The claim records which attachment owns it, and every
  release or extension is a compare-and-append that checks that owner, so a
  stale detach or renew from a replaced attachment cannot touch its
  successor's claim; attach reserves first and commits only after the claim
  exists, and crash leftovers are reconciled on the next session command.
  A replace that crashes after the claim changed hands is undone by handing
  the claim back to the still-live predecessor. Responder hooks treat a
  reserved identity as busy even before its claim exists. Session commands
  refuse to change state while the session log has any unreadable record,
  and the generic `claims release` and `claims refresh` skip claims owned by
  a session attachment. The synced claim carries no harness session id.
  Attach refuses an ownerless `agent://` claim held by its own agent rather
  than adopting it: that is what a running responder holds, and refusing it
  is what makes hook admission and attachment safe across the two logs.
  Readability of each log is judged on the same locked snapshot as the
  append. A launcher can `sessions reserve` the identity before its harness
  exists and bind the session id afterwards with `attach --attachment`, so
  no responder can start in the gap. Responder hooks read the reservation
  inside their locked claim stake. Every occupancy transition enters the
  sync auto-commit path like any other claim. A takeover that fails or
  times out after the claim changed hands gives it back to the live
  predecessor instead of releasing it. A direct `attach` cannot protect a
  harness that was started before any claim existed, and says so when a
  responder won the identity first; launchers that must never overlap use
  `reserve` before starting the harness. Binding a reservation verifies its
  claim is still live and owned by it, and a reservation's claim must
  outlive its window. A lifecycle event this build does not understand
  keeps the identity reserved and cannot be detached by an older binary.
  Occupancy stays advisory, as every rite claim is: a concurrent `sync pull`
  can replace `claims.jsonl` under a held lock, and phase one accepts that
  rather than serialising sync with storage writes.
  Attachments live in `local/sessions.jsonl`, which
  `sync init` ignores and `sync push` excludes by pathspec, since a session
  id means nothing on another host. `rite agents` shows the attachment.
  Design and test evidence: `notes/agent-sessions.md`.

### Fixed

- **A trigger queued behind a spawn lease is no longer stranded when the
  channel goes quiet.** The lease batches triggers that arrive while a spawn is
  live and hands them to the next spawn, but nothing scheduled that next spawn:
  it happened only when a later message fired the same hook. On `#console` a
  review approval sat undelivered for two days behind a lease that had lapsed
  twenty minutes after it was queued. Every command that evaluates hooks now
  re-checks pending queues in **every** channel, so a `rite send` in `#maw`
  delivers a trigger stranded in `#console`. No daemon and no timer: the drain
  rides traffic that already exists. A swept spawn is indistinguishable from a
  message-driven one — `RITE_CHANNEL` is the channel the trigger came from
  rather than whichever channel was busy, and the batch stays chronological
  with the newest trigger as its anchor.
- A leased hook no longer queues messages it was never addressed by. The lease
  is taken before the condition is evaluated, which it has to be, so a mention
  hook queued *every* message in its channel for as long as a spawn was live —
  handing the next spawn a batch that was mostly not its work. Channel hooks
  are unaffected: any message in the channel is what addresses them.
- `rite hooks remove` retires whatever is queued for that hook. Every delivery
  path matches on the hook id, so a queue outliving its hook could never be
  delivered by anything.

### Added

- `rite hooks drain` forces the sweep, and `--dry-run` reports what is stranded
  without taking a lease or spawning anything. The sweep needs traffic to ride;
  this is the escape hatch for a machine where nothing is talking to rite, and
  the way to explain a responder that appears not to have woken up.
- `rite hooks drain --discard` retires a stranded queue instead of delivering
  it, for a backlog no longer worth acting on: a responder that was down while
  its channel stayed busy returns to as many as 500 queued triggers and works
  through them 50 per spawn. Previously the only ways out were `hooks remove`,
  which retires the queue but takes the hook ID and so breaks the spawn lease,
  or hand-editing `hook_queue.jsonl`. Requires `--hook-id` or `--all`, since a
  discarded queue does not come back — though the messages do, being durable in
  their channel all along. Named `--discard` rather than a `hooks clear` verb,
  which reads equally as "delete every hook".

## [0.34.0] - 2026-08-12

Hooks you can change without destroying, and a doctor that notices when one
cannot possibly run.

### Changed

- **`rite history` hides system messages by default.** Hook firings, agent
  registrations, and claim expiries are withheld unless you pass
  `--show-system`. On a busy channel these are a fifth to nearly half of every
  read: measured over the last 500 messages, `#console` was 44% hook-fired
  system lines, `#wraith` 33%, `#maw` 21%, `#rite` 18%. Nothing is hidden
  silently — text output ends with `N system messages hidden (--show-system)`,
  and JSON carries `hidden_system` plus an `advice` entry. `-n` counts readable
  messages, and `--from system` and `--thread` include them without the flag.
  Claim records are deliberately not treated as system messages: they are the
  entire content of `#claims`.
- **`rite ui` hides system messages by default**, matching `history`. `ctrl+h`
  brings them back. That toggle no longer hides claim records, so `#claims`
  does not open on an empty screen.
- `rite hooks add --name <key>` now **updates** an existing hook with that name
  on that channel instead of creating a second one. Nothing passes `--name`
  yet, so no existing hook changes behaviour.

### Added

- `rite hooks set <id>` changes a hook in place, keeping its ID. Every field
  you do not name keeps its value, including fields this build does not
  understand. Previously any change meant `hooks remove` followed by
  `hooks add`, which is not an equivalent operation: the hook ID is the
  spawn-lease key (`spawn://<id>/<channel>`), so a new ID leaves a running
  spawn holding a lease nobody checks and lets the replacement spawn a second
  agent beside it. It also cleared `last_fired`, handing a cooldown hook an
  immediate free firing, and dropped any field the caller did not re-type.
- `rite hooks add --name` and `--owner`, giving a hook a stable identity an
  external tool can converge on, plus `rite hooks list --owner <tool>`.
  A converge preserves anything it does not name — including the lease — so a
  manager that has never heard of `--lease` can no longer strip one. Turning a
  lease off stays deliberate: `rite hooks set <id> --no-lease`.
- `rite doctor` reports hooks that cannot run: a `cwd` that no longer exists,
  or a command that is not on PATH. Warning, not failure, since a hook for a
  project checked out on another machine is legitimate. Eight of forty-two
  live hooks were in this state while doctor reported a healthy environment;
  one had fired against a deleted directory 228 times. A firing that fails to
  spawn records `executed: false`, exactly like a cooldown skip, so nothing
  else distinguishes a dead hook from a quiet one.
- `rite doctor` reports a data directory whose git store is broken. Sync
  commits into that repository on every write, so a corrupt store means every
  commit fails silently while the JSONL — the actual source of truth — stays
  correct. That went unnoticed for about 2.5 days after an unclean shutdown,
  with doctor reporting healthy throughout.

## [0.33.0] - 2026-08-11

Threading, finished. 0.32.0 could record that a message answers another one;
this release lets you block on that answer and read it as an answer.

### Added

- `rite wait --reply-to <id>` blocks until someone answers a specific message,
  so a request no longer has to be posted and guessed about. Exit 0 means
  answered, 1 means nobody replied inside the timeout, 2 means the id is not a
  ULID or this store never saw it. `--reply-to` narrows rather than widens: it
  names one question, and `--from`, `-c`, and `-L` only remove candidate
  answers from it. Your own reply does not acknowledge you. A reply that landed
  before the wait started is still reported, so there is no race between `rite
  send` and `rite wait`. Use `--allow-missing-parent` when the parent is still
  syncing in from another machine.
- `rite ui` renders a reply as a reply. Replies indent under a connector,
  carry a `↩ reply` badge, and show a one-line preview of the parent, so an
  answer to a message far up the transcript reads as an answer rather than a
  non-sequitur. Nesting is capped at four visual levels; deeper replies stay
  legible and report their true depth. A parent that is missing, tombstoned, a
  self-reference, or part of a cycle is badged as such instead of being drawn
  as an ordinary reply.

### Fixed

- `scripts/screenshot-tui.sh` works under niri, and picks its compositor from
  the IPC handle rather than from which binaries are installed — `hyprctl` is
  frequently present on machines not running Hyprland, and it exits 0 even when
  it cannot reach a compositor. With no supported compositor the script now
  fails immediately and points at `vessel`. Output is converted with
  ImageMagick to `images/tui.webp`, which is the file the README actually
  references; the script previously wrote a `.png` nothing used.

### Documentation

- `.agents/tui-screenshot.md` separates the two jobs it used to conflate:
  `vessel` verifies a TUI change and needs no compositor, so it works over SSH
  and in sandboxes; `screenshot-tui.sh` exists only to regenerate the README
  image. Includes the vessel command table, and the two failure modes that read
  exactly like a change that did not land — a stale `target/release/rite`, and
  a missing `RITE_DATA_DIR` pointing the TUI at the live hook fleet.

## [0.32.0] - 2026-08-09

### Added

- `rite mentions follow` — a single-process JSONL stream of every message that
  mentions you, across all channels, plus your DMs. Replaces the one-watcher-
  per-channel approach: one inotify watch, incremental reads with per-channel
  offsets, and constant memory as channels accumulate. Your own DMs stream by
  default; pass `--no-dms` for a mentions-only stream. A mention never routes a
  message out of a DM you are not part of.
- Message threading. `Message` carries an optional `reply_to`; `rite send
  --reply-to <id>` anchors a reply and `rite history --thread <id>` retrieves a
  thread. Missing, unsynced, and tombstoned parents degrade to a labelled
  fragment instead of a silent reparent. Self-references and cycles terminate.
- `rite claims list` and `rite claims check` report a `stale` flag when the
  claim holder's presence has lapsed, so a claim held by a dead agent is
  visible as such. Staleness is a report: nothing auto-releases another agent's
  claim. Presence is derived from activity, with a TTL of three heartbeat
  intervals so one missed beat does not flap an agent offline.
- Hook spawn leases (`rite hooks add --lease`). One live spawn per hook and
  channel, with triggers arriving during a turn batched and deduplicated for
  the next spawn instead of dropped or spawned per message. Opt-in; existing
  cooldown hooks are unchanged. A lease whose holder has provably gone away
  does not block forever.
- `rite send` gained `--format`. JSON and text output now report the new
  message id.

### Fixed

- JSONL readers no longer abort a whole file over one unreadable record. A
  record whose type this build does not recognize is read and preserved; a
  record whose type is known but whose body does not fit is reported as
  damaged. `rite doctor` reports both counts separately, so a future format and
  real corruption are never confused.
- Hook records preserve fields this build does not understand across a rewrite,
  so an older rite firing a hook can no longer silently erase newer
  configuration such as a spawn lease.
- `AGENTS.md` documented a `rite wait --mention` flag that does not exist. The
  flag is `--mentions`.

### Changed

- **`rite send` text output changed.** It previously printed
  `Sent: Message sent to #channel`; it now leads with `id: <ulid>`. Scripts
  parsing the old string need updating. Interactive (TTY) output keeps the
  confirmation line and adds the id.

## [0.31.3] - 2026-06-16

### Fixed

- `history -f` (follow mode) no longer drops messages. The follow cursor is now
  seeded from the initial bounded read's `next_offset` instead of the file's
  current EOF, and any startup backlog is drained before the event loop. With
  `--after-offset` plus a count limit, messages between the bounded read and EOF
  were previously skipped.
- `history --after-id <id> -n <count>` now reports a correct `next_offset`.
  Previously the result was truncated to `count` but `next_offset` pointed at
  EOF, so paginating with `--after-offset` skipped every message between the
  count-th returned message and EOF. `--after-id` now resolves to a byte offset
  and shares the lossless offset-based read path, and the pagination advice hint
  works for both `--after-offset` and `--after-id`.

## [0.31.2] - 2026-05-01

### Added

- `wait --from` conversation filter.

### Fixed

- Fresh-eyes bug sweep.
