# botbus

Project type: cli
Tools: `beads`, `maw`, `crit`, `botbus`, `botty`
Reviewer roles: security

<!-- Add project-specific context below: architecture, conventions, key files, etc. -->

**ALWAYS run these commands before committing and pushing:**

```bash
# 1. Format code
cargo fmt

# 2. Fix clippy warnings
cargo clippy -- -D warnings

# 3. Run tests
just test
```

**If you skip these steps, CI will fail and the user will get error emails.**

These checks are enforced in CI, so any formatting issues or clippy warnings will cause the build to fail. Running them locally first prevents unnecessary CI failures and email notifications.

---

## Agent Communication

This project uses BotBus for agent coordination. BotBus uses global storage (~/.local/share/botbus/) shared across all projects.

### Quick Start

```bash
# Set your identity

# Recommended: Use --agent flag (works in all environments, including sandboxed)
bus --agent my-agent status
bus --agent my-agent send general "message"

# Alternative: Use env var (only persists in continuous shell sessions)
export BOTBUS_AGENT=$(bus generate-name)  # e.g., "swift-falcon"
bus status  # Uses BOTBUS_AGENT

# Note: In sandboxed environments (like Claude Code), env vars don't persist
# between commands. Use --agent flag for each command in these environments.

# Check what's happening
bus status              # Overview: agents, channels, claims
bus history             # Recent messages in #general
bus agents              # Who's been active

# Communicate
bus send general "Starting work on X"
bus send general "Done with X, ready for review"
bus send @other-agent "Question about Y"

# Coordinate file access (claims use absolute paths internally)
bus claim "src/api/**" -m "Working on API routes"
bus check-claim src/api/routes.rs   # Check before editing
bus release --all                    # When done

# Claim non-file resources (issues, ports, etc.)
bus claim "bead://botbus/bd-123" -m "Working on this issue"
bus check-claim "bead://botbus/bd-123"
```

### Best Practices

1. **Use --agent flag** or set BOTBUS_AGENT (stateless, doesn't persist across sandboxed commands)
2. **Run `bus status`** to see current state before starting work
3. **Claim files** you plan to edit - overlapping claims are denied
4. **Check claims** before editing files outside your claimed area
5. **Send updates** on blockers, questions, or completed work
6. **Release claims** when done - don't hoard files

### Channel Conventions

- `#general` - Default channel for cross-project coordination
- `#project-name` - Project-specific updates (e.g., `#botbus`, `#webapp`)
- `#project-topic` - Sub-topics (e.g., `#botbus-tui`, `#webapp-api`)
- `@agent-name` - Direct messages for specific coordination

Channel names: lowercase alphanumeric with hyphens (e.g., `my-channel`, not `my.channel`)

### Message Conventions

Keep messages concise and actionable:
- "Starting work on bd-xyz: Add foo feature"
- "Blocked: need database credentials to proceed"
- "Question: should auth middleware go in src/api or src/auth?"
- "Done: implemented bar, tests passing"

### Waiting for Replies

```bash
# After sending a DM, wait for reply
bus send @other-agent "Can you review this?"
bus wait -c @other-agent -t 60  # Wait up to 60s for reply

# Wait for any @mention of you
bus wait --mention -t 120
```

---

## CLI Output for AI Agents

BotBus outputs are designed to be parsed by AI agents. Commands default to TOON format (token-efficient key-value structure) and support `--format json` and `--format text`.

**For detailed guidance on interpreting command output**, see [.agents/cli-output.md](.agents/cli-output.md), which covers:
- Output format differences (TOON, JSON, Text)
- Common field meanings and what actions they suggest
- Time specifications (relative like "2h ago", absolute like "2026-01-30")
- Error patterns and troubleshooting hints
- Empty results and exit codes

---

## Development Notes

- Run `just test` before committing
- Agent identity flows via `BOTBUS_AGENT` env var or `--agent` flag (stateless)
- Claims stored with absolute paths, displayed relative when in same directory tree

### Version Control: jj (Jujutsu) with Git

This repo uses jj colocated with git. jj creates commits on a "floating" working copy, not directly on branches. **You must move the `main` bookmark after committing to ensure changes can be pushed to GitHub.**

#### First-Time Setup

```bash
# Track the main bookmark with origin (one-time setup)
jj bookmark track main --remote=origin
```

#### Commit Workflow

```bash
# 1. Make your changes and commit
jj commit -m "feat(scope): description

Co-Authored-By: Claude <noreply@anthropic.com>"

# 2. Move main bookmark to include your commit
#    @- means "parent of working copy" (your just-created commit)
jj bookmark set main -r @-

# 3. Verify main is ahead of origin
jj log --limit 3
# Should show: main (your commit) ahead of main@origin

# 4. Push to GitHub
jj git push
# IMPORTANT: Despite the output saying "Changes to push to origin:",
# the push ALREADY HAPPENED. Do NOT run git push afterwards.
# Verify with: git log origin/main --oneline -1
```

#### If You Forget to Move the Bookmark

If you committed but forgot to move `main`, find your commit and move the bookmark:

```bash
# Find your commit
jj log

# Move main to it (use the change ID like 'opzoplvm' or revision like '6e2a5dff')
jj bookmark set main -r <change-id>

# Then push
jj git push
```

#### Quick Reference

| Task | Command |
|------|---------|
| See current state | `jj log --limit 5` |
| Commit changes | `jj commit -m "message"` |
| Move main to last commit | `jj bookmark set main -r @-` |
| Push to GitHub | `jj git push` (output looks like preview but actually pushes!) |
| Verify push succeeded | `git log origin/main --oneline -1` |
| Sync from GitHub | `jj git fetch` then `jj rebase -d main@origin` |

### Commit Conventions

Use [semantic commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

Co-Authored-By: Claude <noreply@anthropic.com>
```

**Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

**Scopes**: `cli`, `tui`, `storage`, `core`, `test`, etc.

**Always include** the `Co-Authored-By` trailer when AI assists with commits.

Examples:
- `feat(tui): add mouse support and help overlay`
- `fix(storage): hold lock across read-modify-write in state update`
- `docs: update README with new screenshot`
- `refactor(cli): extract claim validation logic`

## Development Workflow

This section covers the full cycle: creating a feature branch, implementing changes, getting review, and releasing.

### 1. Start a Feature Branch

```bash
# Create a new commit for your work
jj new -m "wip: description of change"

# Create a bookmark for the feature
jj bookmark create feature-name

# Work on your changes...
jj describe -m "feat(scope): description of change"
```

### 2. Request Code Review

After completing your changes and ensuring tests pass:

```bash
# Verify build and tests
just build && just test

# Create a review
crit reviews create --title "feat(scope): description of change"
# Note the review ID (e.g., cr-xxxx)
```

**Spawn specialist reviewers** using the code-review skill (`~/.claude/skills/code-review/SKILL.md`):

- **Security reviewer** (always): Looks for injection, auth issues, resource exhaustion, etc.
- **Architecture reviewer** (for structural changes): Evaluates design, abstractions, maintainability

The skill has ready-to-use prompts for spawning these subagents.

### 3. Address Review Feedback

Monitor bus for reviewer completion:

```bash
bus history general
```

For each thread raised:

```bash
# View threads
crit threads list <review_id>
crit threads show <thread_id>

# Respond (specify your agent identity with --agent flag)
crit --agent <your-agent> comments add <thread_id> "Response explaining fix or rationale"

# After addressing, resolve with reason
crit threads resolve <thread_id> --reason "Fixed: description"
crit threads resolve <thread_id> --reason "Won't fix: rationale"
crit threads resolve <thread_id> --reason "Deferred: created bead bd-xxx"
```

### 4. Get Approval

Reviewers vote with:

```bash
crit lgtm <review_id> -m "Reason"    # Approve
crit block <review_id> -r "Reason"   # Block
```

### 5. Merge and Release

Once approved (LGTM votes, no blocking votes, all threads resolved):

```bash
# Approve and merge the review
crit reviews approve <review_id>
crit reviews merge <review_id>

# Bump version in Cargo.toml (edit manually or with sed)
# e.g., 0.2.0 to 0.3.0

# Update commit message
jj describe -m "chore: bump version to X.Y.Z

Co-Authored-By: Claude <noreply@anthropic.com>"

# Move main bookmark forward and push
jj bookmark set main -r @
jj git push --bookmark main

# Tag the release and push tag
jj tag set vX.Y.Z -r main
git push origin vX.Y.Z

# Install locally
just install

# Verify
bus --version

# Announce on botbus
bus --agent <your-agent> send bus"Released vX.Y.Z - [summary of changes]"
```

### Quick Reference

| Stage | Key Commands |
|-------|--------------|
| Start feature | `jj new -m "wip: ..."` then `jj bookmark create name` |
| Create review | `crit reviews create --title "..."` |
| View threads | `crit threads list <review_id>` |
| Respond | `crit comments add <thread_id> "..."` |
| Resolve | `crit threads resolve <thread_id> --reason "..."` |
| Approve/merge | `crit reviews approve <id> && crit reviews merge <id>` |
| Release | bump version -> `jj bookmark set main` -> push -> tag -> `just install` |

---

## Tools

### Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) for issue tracking. Issues are stored in `.beads/` and tracked in git.

**Note:** `br` (beads_rust) is non-invasive and never executes git commands directly. After running `br sync --flush-only`, you must manually run git commands to commit and push changes.

#### Essential Commands

```bash
# View issues (launches TUI - avoid in automated sessions)
bv

# CLI commands for agents (use these instead)
br ready              # Show issues ready to work (no blockers)
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br create --title="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason="Completed"
br close <id1> <id2>  # Close multiple issues at once
br sync --flush-only  # Export to JSONL (does NOT run git commands)
git add .beads/ && git commit -m "Update beads" && git push  # Manual git steps
```

#### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Run `br sync --flush-only`, then manually `git add .beads/ && git commit && git push`

#### Issue Quality

When creating or updating issues, always include:
- **Description**: What the issue is about, context, and acceptance criteria
- **Labels**: Use `--add-label` to categorize (e.g., `cli`, `agent-ux`, `data-model`, `bug`, `enhancement`)

```bash
br create --title="Add foo feature" --type=task --priority=2
br update <id> --description="Detailed description here" --add-label=cli --add-label=enhancement
```

### Using bv as an AI sidecar

bv is a fast terminal UI for Beads projects (.beads/issues.jsonl). It renders lists/details and precomputes dependency metrics (PageRank, critical path, cycles, etc.) so you instantly see blockers and execution order. Source of truth here is `.beads/issues.jsonl` (exported from `beads.db`); legacy `.beads/beads.jsonl` is deprecated and must not be used. For agents, it’s a graph sidecar: instead of parsing JSONL or risking hallucinated traversal, call the robot flags to get deterministic, dependency-aware outputs.

- bv --robot-help — shows all AI-facing commands.
- bv --robot-insights — JSON graph metrics (PageRank, betweenness, HITS, critical path, cycles) with top-N summaries for quick triage.
- bv --robot-plan — JSON execution plan: parallel tracks, items per track, and unblocks lists showing what each item frees up.
- bv --robot-priority — JSON priority recommendations with reasoning and confidence.
- bv --robot-recipes — list recipes (default, actionable, blocked, etc.); apply via bv --recipe <name> to pre-filter/sort before other flags.
- bv --robot-diff --diff-since <commit|date> — JSON diff of issue changes, new/closed items, and cycles introduced/resolved.

Use these commands instead of hand-rolling graph logic; bv already computes the hard parts so agents can act safely and quickly.

### ast-grep vs ripgrep (quick guidance)

**Use `ast-grep` when structure matters.** It parses code and matches AST nodes, so results ignore comments/strings, understand syntax, and can **safely rewrite** code.

- Refactors/codemods: rename APIs, change import forms, rewrite call sites or variable kinds.
- Policy checks: enforce patterns across a repo (`scan` with rules + `test`).
- Editor/automation: LSP mode; `--json` output for tooling.

**Use `ripgrep` when text is enough.** It’s the fastest way to grep literals/regex across files.

- Recon: find strings, TODOs, log lines, config values, or non‑code assets.
- Pre-filter: narrow candidate files before a precise pass.

**Rule of thumb**

- Need correctness over speed, or you’ll **apply changes** → start with `ast-grep`.
- Need raw speed or you’re just **hunting text** → start with `rg`.
- Often combine: `rg` to shortlist files, then `ast-grep` to match/modify with precision.

**Snippets**

Find structured code (ignores comments/strings):

```bash
ast-grep run -l TypeScript -p 'import $X from "$P"'
```

Codemod (only real `var` declarations become `let`):

```bash
ast-grep run -l JavaScript -p 'var $A = $B' -r 'let $A = $B' -U
```

Quick textual hunt:

```bash
rg -n 'console\.log\(' -t js
```

Combine speed + precision:

```bash
rg -l -t ts 'useQuery\(' | xargs ast-grep run -l TypeScript -p 'useQuery($A)' -r 'useSuspenseQuery($A)' -U
```

**Mental model**

- Unit of match: `ast-grep` = node; `rg` = line.
- False positives: `ast-grep` low; `rg` depends on your regex.
- Rewrites: `ast-grep` first-class; `rg` requires ad‑hoc sed/awk and risks collateral edits.

### TUI Screenshot

When making visual changes to the TUI, update the README screenshot:

```bash
./scripts/screenshot-tui.sh           # Captures 1200x800 to images/tui.png
./scripts/screenshot-tui.sh 1600 900  # Custom dimensions
```

Requires: Hyprland, kitty, grim, pngquant. The script spawns a floating window, captures it, and compresses the image.


<!-- botbox:managed-start -->
## Botbox Workflow

**New here?** Read [worker-loop.md](.agents/botbox/worker-loop.md) first — it covers the complete triage → start → work → finish cycle.

**All tools have `--help`** with usage examples. When unsure, run `<tool> --help` or `<tool> <command> --help`.

### Beads Quick Reference

| Operation | Command |
|-----------|---------|
| View ready work | `br ready` |
| Show bead | `br show <id>` |
| Create | `br create --actor $AGENT --owner $AGENT --title="..." --type=task --priority=2` |
| Start work | `br update --actor $AGENT <id> --status=in_progress` |
| Add comment | `br comments add --actor $AGENT --author $AGENT <id> "message"` |
| Close | `br close --actor $AGENT <id>` |
| Add dependency | `br dep add --actor $AGENT <blocked> <blocker>` |
| Sync | `br sync --flush-only` |

**Required flags**: `--actor $AGENT` on mutations, `--author $AGENT` on comments.

### Beads Conventions

- Create a bead before starting work. Update status: `open` → `in_progress` → `closed`.
- Post progress comments during work for crash recovery.
- **Push to main** after completing beads (see [finish.md](.agents/botbox/finish.md)).

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

Use `@<project>-<role>` mentions to request reviews:

```bash
crit reviews request <review-id> --reviewers $PROJECT-security --agent $AGENT
bus send --agent $AGENT $PROJECT "Review requested: <review-id> @$PROJECT-security" -L review-request
```

The @mention triggers the auto-spawn hook for the reviewer.

### Cross-Project Communication

When you have questions, feedback, or issues with tools from other projects:

1. Find the project: `bus inbox --agent $AGENT --channels projects --all`
2. Post to their channel: `bus send <project> "..." -L feedback`
3. For bugs/features, create beads in their repo (see [report-issue.md](.agents/botbox/report-issue.md))

This includes: bugs, feature requests, confusion about APIs, UX problems, or just questions.


### Design Guidelines

- [CLI tool design for humans, agents, and machines](.agents/botbox/design/cli-conventions.md)

### Workflow Docs

- [Close bead, merge workspace, release claims, sync](.agents/botbox/finish.md)
- [groom](.agents/botbox/groom.md)
- [Verify approval before merge](.agents/botbox/merge-check.md)
- [Validate toolchain health](.agents/botbox/preflight.md)
- [Report bugs/features to other projects](.agents/botbox/report-issue.md)
- [Reviewer agent loop](.agents/botbox/review-loop.md)
- [Request a review](.agents/botbox/review-request.md)
- [Handle reviewer feedback (fix/address/defer)](.agents/botbox/review-response.md)
- [Claim bead, create workspace, announce](.agents/botbox/start.md)
- [Find work from inbox and beads](.agents/botbox/triage.md)
- [Change bead status (open/in_progress/blocked/done)](.agents/botbox/update.md)
- [Full triage-work-finish lifecycle](.agents/botbox/worker-loop.md)
<!-- botbox:managed-end -->
