# botbus

Project type: Rust CLI (`cargo`)
Tools: `beads`, `maw`, `crit`, `botbus`, `botty`
Reviewer roles: security

## What This Is

Chat-oriented coordination CLI for AI coding agents. When multiple agents work on the same codebase — or across projects — they need a way to communicate, claim resources, and stay out of each other's way. botbus provides that with zero infrastructure.

**Design principles:**
- **Zero infrastructure** — append-only JSONL on disk. No daemon, no server, no ports, no database.
- **Agent-first, human-friendly** — every command works headlessly with structured output (TOON/JSON/text). Humans get `bus ui`.
- **Claims for anything** — file globs, URIs (`bead://`, `db://`), ports — any string. Advisory locks, not enforced.
- **Append-only** — JSONL files are the source of truth. SQLite indexes are derived and rebuildable (`bus index rebuild`).
- **Convention over configuration** — sensible defaults, minimal setup. `bus send general "hello"` just works.

**Architecture:** Single binary (`bus`). Storage at `~/.local/share/botbus/` — channels are `channels/<name>.jsonl`, claims in `claims.jsonl`, agent state in SQLite (derived). Telegram bridge (`bus telegram`) runs as a long-lived process. TUI (`bus ui`) is a separate mode.

**Scope boundaries — botbus is a coordination primitive.** It is NOT a task runner, CI system, build tool, or general-purpose message queue. Push back on scope creep into: job scheduling, build automation, git operations beyond sync, file editing/patching, or process management.

---

## Pre-commit Checks

```bash
cargo fmt && cargo clippy -- -D warnings && just test
```

CI enforces these. Skipping them causes build failures and error emails.

---

## CLI Reference

All commands support `--agent <name>` (or `BOTBUS_AGENT` env var), `--format toon|json|text`, `-q` (quiet), `-v` (verbose).

### Core

| Command | Usage |
|---------|-------|
| `send` | `bus send <target> <message> [-L label] [--attach file] [--no-hooks]` |
| `history` | `bus history [channel] [-n count] [-f] [--since/--before] [--from] [-L label]` |
| `inbox` | `bus inbox [-c channels] [--all] [--mentions] [-n count] [--mark-read] [--count-only]` |
| `mark-read` | `bus mark-read <channel>` |
| `search` | `bus search <query> [-c channel] [-n count] [--from]` |
| `wait` | `bus wait [-c channel] [--mention] [-L label] [-t timeout]` |
| `watch` | `bus watch [channel]` — stream messages in real-time |
| `status` | `bus status` — overview of agents, channels, claims |

### Claims (advisory locks)

| Command | Usage |
|---------|-------|
| `claims stake` | `bus claims stake <patterns...> [-t ttl] [-m message]` |
| `claims check` | `bus claims check <path>` |
| `claims release` | `bus claims release [patterns...] [--all]` |
| `claims list` | `bus claims list [--all] [--mine] [-n limit]` |
| `claims refresh` | `bus claims refresh` — extend TTL |

### Management

| Command | Usage |
|---------|-------|
| `agents` | `bus agents [--active]` |
| `channels` | `bus channels list\|close\|reopen\|delete\|rename` |
| `hooks` | `bus hooks add\|list\|remove\|test` |
| `subscriptions` | `bus subscriptions add\|remove\|list` |
| `statuses` | `bus statuses set\|clear\|list` |
| `messages` | `bus messages get <id>` |

### Sync & Infra

| Command | Usage |
|---------|-------|
| `sync` | `bus sync init\|push\|pull\|status\|log\|check` |
| `index` | `bus index rebuild\|status` |
| `telegram` | `bus telegram` — run Telegram bridge |
| `ui` | `bus ui [-c channel]` — terminal UI |
| `init` | `bus init` — create data directory |
| `doctor` | `bus doctor` — check environment health |
| `generate-name` | `bus generate-name` — random kebab-case name |
| `whoami` | `bus whoami` |

### Attachments

```bash
bus send general "see attached" --attach ./screenshot.png
bus send general "link" --attach https://example.com/file.tar.gz
bus send general "named" --attach "label:./path/to/file"
```

Files are stored in a content-addressed cache (SHA256). The Telegram bridge relays attachments bidirectionally.

### Message Flags

Inline flags in message body suppress hook execution:
- `!nohooks` — suppress all hooks
- `!nochanhooks` — suppress channel hooks only
- `!noathooks` — suppress @-mention hooks only

Example: `bus send general "deploy done !nohooks"`

Alternatively, use `--no-hooks` on the CLI.

---

## Agent Communication

### Identity

```bash
# Recommended: --agent flag (works in sandboxed environments)
bus --agent my-agent send general "hello"

# Alternative: env var (doesn't persist across sandboxed commands)
export BOTBUS_AGENT=$(bus generate-name)
```

### Quick Start

```bash
bus status                                    # What's happening?
bus send general "Starting work on X"         # Announce
bus send @other-agent "Question about Y"      # DM
bus claims stake "src/api/**" -m "Working on API"  # Claim files
bus claims check src/api/routes.rs            # Check before editing
bus claims release --all                      # Release when done
bus wait -c @other-agent -t 60               # Wait for reply
```

### Channel Conventions

- `#general` — cross-project coordination
- `#project-name` — project-specific (e.g., `#botbus`)
- `@agent-name` — direct messages

Names: lowercase alphanumeric with hyphens.

### Message Style

Keep messages concise and actionable:
- "Starting work on bd-xyz: Add foo feature"
- "Blocked: need database credentials to proceed"
- "Done: implemented bar, tests passing"

---

## Development Notes

- Storage: `~/.local/share/botbus/` (override with `BOTBUS_DATA_DIR`)
- Identity: `BOTBUS_AGENT` env var or `--agent` flag
- Claims stored with absolute paths, displayed relative when in same directory tree
- Git sync disables GPG signing in data repos automatically
- JSONL is append-only; indexes derived via `bus index rebuild`

### Output Formats

Commands default to TOON (token-efficient for agents). Use `--format json` for structured parsing or `--format text` for human reading. See [.agents/cli-output.md](.agents/cli-output.md) for detailed format guidance.

### Further Reading

- [Testing strategy and test harness](.agents/testing.md)
- [TUI screenshot workflow](.agents/tui-screenshot.md)
- [CLI output format details](.agents/cli-output.md)

---

## Tools

### Beads (Issue Tracking)

Uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust). Issues in `.beads/`, tracked in git. `br` never runs git commands — after `br sync --flush-only`, manually commit and push.

```bash
br ready                          # Actionable work
br show <id>                      # Full details
br create --title="..." --type=task --priority=2
br close <id>
```

### bv (Beads Viewer)

Fast TUI for `.beads/issues.jsonl` with precomputed dependency metrics. For agents, use the robot flags instead of parsing JSONL:

- `bv --robot-help` — all AI-facing commands
- `bv --robot-plan` — execution plan with parallel tracks
- `bv --robot-priority` — priority recommendations
- `bv --robot-insights` — graph metrics (PageRank, critical path, cycles)

---

<!-- botbox:managed-start -->
## Botbox Workflow

**New here?** Read [worker-loop.md](.agents/botbox/worker-loop.md) first — it covers the complete triage → start → work → finish cycle.

**All tools have `--help`** with usage examples. When unsure, run `<tool> --help` or `<tool> <command> --help`.

### IMPORTANT: Always Track Work in Beads

**Every non-trivial change MUST have a bead**, no matter how it originates:
- **User asks you to do something** → create a bead before starting
- **You propose a change** → create a bead before starting
- **Mid-conversation pivot to implementation** → create a bead before coding

The only exceptions are truly microscopic changes (typo fixes, single-line tweaks) or when you are already iterating on an existing bead's implementation.

Without a bead, work cannot be recovered from crashes, handed off to other agents, or tracked for review. When in doubt, create the bead — it takes seconds and prevents lost work.

### Directory Structure (maw v2)

This project uses a **bare repo** layout. Source files live in workspaces under `ws/`, not at the project root.

```
project-root/          ← bare repo (no source files here)
├── ws/
│   ├── default/       ← main working copy (AGENTS.md, .beads/, src/, etc.)
│   ├── frost-castle/  ← agent workspace (isolated jj commit)
│   └── amber-reef/    ← another agent workspace
├── .jj/               ← jj repo data
├── .git/              ← git data (core.bare=true)
├── AGENTS.md          ← stub redirecting to ws/default/AGENTS.md
└── CLAUDE.md          ← symlink → AGENTS.md
```

**Key rules:**
- `ws/default/` is the main workspace — beads, config, and project files live here
- **Never merge or destroy the default workspace.** It is where other branches merge INTO, not something you merge.
- Agent workspaces (`ws/<name>/`) are isolated jj commits for concurrent work
- **ALL commands must go through `maw exec`** — this includes `br`, `bv`, `crit`, `jj`, `cargo`, `bun`, and any project tool. Never run them directly from the bare repo root.
- Use `maw exec default -- <command>` for beads (`br`, `bv`) and general project commands
- Use `maw exec <agent-ws> -- <command>` for workspace-scoped commands (`crit`, `jj describe`, `cargo check`)
- **crit commands must run in the review's workspace**, not default: `maw exec <ws> -- crit ...`

### Beads Quick Reference

| Operation | Command |
|-----------|---------|
| View ready work | `maw exec default -- br ready` |
| Show bead | `maw exec default -- br show <id>` |
| Create | `maw exec default -- br create --actor $AGENT --owner $AGENT --title="..." --type=task --priority=2` |
| Start work | `maw exec default -- br update --actor $AGENT <id> --status=in_progress --owner=$AGENT` |
| Add comment | `maw exec default -- br comments add --actor $AGENT --author $AGENT <id> "message"` |
| Close | `maw exec default -- br close --actor $AGENT <id>` |
| Add dependency | `maw exec default -- br dep add --actor $AGENT <blocked> <blocker>` |
| Sync | `maw exec default -- br sync --flush-only` |
| Triage (scores) | `maw exec default -- bv --robot-triage` |
| Next bead | `maw exec default -- bv --robot-next` |

**Required flags**: `--actor $AGENT` on mutations, `--author $AGENT` on comments.

### Workspace Quick Reference

| Operation | Command |
|-----------|---------|
| Create workspace | `maw ws create <name>` |
| List workspaces | `maw ws list` |
| Merge to main | `maw ws merge <name> --destroy` |
| Destroy (no merge) | `maw ws destroy <name>` |
| Run jj in workspace | `maw exec <name> -- jj <jj-args...>` |

**Avoiding divergent commits**: Each workspace owns ONE commit. Only modify your own.

| Safe | Dangerous |
|------|-----------|
| `maw ws merge <agent-ws> --destroy` | `maw ws merge default --destroy` (NEVER) |
| `jj describe` (your working copy) | `jj describe main -m "..."` |
| `maw exec <your-ws> -- jj describe -m "..."` | `jj describe <other-change-id>` |

If you see `(divergent)` in `jj log`:
```bash
jj abandon <change-id>/0   # keep one, abandon the divergent copy
```

**Working copy snapshots**: jj auto-snapshots your working copy before most operations (`jj new`, `jj rebase`, etc.). Edits go into the **current** commit automatically. To put changes in a **new** commit, run `jj new` first, then edit files.

**Always pass `-m`**: Commands like `jj commit`, `jj squash`, and `jj describe` open an editor by default. Agents cannot interact with editors, so always pass `-m "message"` explicitly.

### Beads Conventions

- Create a bead before starting work. Update status: `open` → `in_progress` → `closed`.
- Post progress comments during work for crash recovery.
- **Push to main** after completing beads (see [finish.md](.agents/botbox/finish.md)).
- **Update CHANGELOG.md** when releasing: add a summary of user-facing changes under the new version heading before tagging.
- **Install locally** after releasing: `just install`

### Identity

Your agent name is set by the hook or script that launched you. Use `$AGENT` in commands.
For manual sessions, use `<project>-dev` (e.g., `myapp-dev`).

### Claims

When working on a bead, stake claims to prevent conflicts:

```bash
bus claims stake --agent $AGENT "bead://<project>/<id>" -m "<id>"
bus claims stake --agent $AGENT "workspace://<project>/<ws>" -m "<id>"
bus claims release --agent $AGENT --all  # when done
```

### Reviews

Create a review with reviewer assignment in one command, then @mention to spawn:

```bash
maw exec $WS -- crit reviews create --agent $AGENT --title "..." --description "..." --reviewers $PROJECT-security
bus send --agent $AGENT $PROJECT "Review requested: <review-id> @$PROJECT-security" -L review-request
```

For re-requests after fixing feedback, use `crit reviews request`:
```bash
maw exec $WS -- crit reviews request <review-id> --reviewers $PROJECT-security --agent $AGENT
```

The @mention triggers the auto-spawn hook for the reviewer.

### Cross-Project Communication

**Don't suffer in silence.** If a tool confuses you or behaves unexpectedly, post to its project channel.

1. Find the project: `bus history projects -n 50` (the #projects channel has project registry entries)
2. Post question or feedback: `bus send --agent $AGENT <project> "..." -L feedback`
3. For bugs, create beads in their repo first
4. **Always create a local tracking bead** so you check back later:
   ```bash
   maw exec default -- br create --actor $AGENT --owner $AGENT --title="[tracking] <summary>" --labels tracking --type=task --priority=3
   ```

See [cross-channel.md](.agents/botbox/cross-channel.md) for the full workflow.

### Session Search (optional)

Use `cass search "error or problem"` to find how similar issues were solved in past sessions.


### Design Guidelines

- [CLI tool design for humans, agents, and machines](.agents/botbox/design/cli-conventions.md)

### Workflow Docs

- [Mission coordination labels and sibling awareness](.agents/botbox/coordination.md)
- [Ask questions, report bugs, and track responses across projects](.agents/botbox/cross-channel.md)
- [Close bead, merge workspace, release claims, sync](.agents/botbox/finish.md)
- [groom](.agents/botbox/groom.md)
- [Verify approval before merge](.agents/botbox/merge-check.md)
- [End-to-end mission lifecycle guide](.agents/botbox/mission.md)
- [Turn specs/PRDs into actionable beads](.agents/botbox/planning.md)
- [Validate toolchain health](.agents/botbox/preflight.md)
- [Create and validate proposals before implementation](.agents/botbox/proposal.md)
- [Report bugs/features to other projects](.agents/botbox/report-issue.md)
- [Reviewer agent loop](.agents/botbox/review-loop.md)
- [Request a review](.agents/botbox/review-request.md)
- [Handle reviewer feedback (fix/address/defer)](.agents/botbox/review-response.md)
- [Explore unfamiliar code before planning](.agents/botbox/scout.md)
- [Claim bead, create workspace, announce](.agents/botbox/start.md)
- [Find work from inbox and beads](.agents/botbox/triage.md)
- [Change bead status (open/in_progress/blocked/done)](.agents/botbox/update.md)
- [Full triage-work-finish lifecycle](.agents/botbox/worker-loop.md)
<!-- botbox:managed-end -->
