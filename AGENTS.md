## ⚠️ CRITICAL: Before Every Push

**ALWAYS run these commands before committing and pushing:**

```bash
# 1. Format code
cargo fmt

# 2. Fix clippy warnings
cargo clippy -- -D warnings

# 3. Run tests
cargo test --all-features
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
botbus --agent my-agent status
botbus --agent my-agent send general "message"

# Alternative: Use env var (only persists in continuous shell sessions)
export BOTBUS_AGENT=$(botbus generate-name)  # e.g., "swift-falcon"
botbus status  # Uses BOTBUS_AGENT

# Note: In sandboxed environments (like Claude Code), env vars don't persist
# between commands. Use --agent flag for each command in these environments.

# Check what's happening
botbus status              # Overview: agents, channels, claims
botbus history             # Recent messages in #general
botbus agents              # Who's been active

# Communicate
botbus send general "Starting work on X"
botbus send general "Done with X, ready for review"
botbus send @other-agent "Question about Y"

# Coordinate file access (claims use absolute paths internally)
botbus claim "src/api/**" -m "Working on API routes"
botbus check-claim src/api/routes.rs   # Check before editing
botbus release --all                    # When done

# Claim non-file resources (issues, ports, etc.)
botbus claim "bead://botbus/bd-123" -m "Working on this issue"
botbus check-claim "bead://botbus/bd-123"
```

### Best Practices

1. **Use --agent flag** or set BOTBUS_AGENT (stateless, doesn't persist across sandboxed commands)
2. **Run `botbus status`** to see current state before starting work
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
botbus send @other-agent "Can you review this?"
botbus wait -c @other-agent -t 60  # Wait up to 60s for reply

# Wait for any @mention of you
botbus wait --mention -t 120
```

---

## Development Notes

- Run `cargo test` before committing
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
# Or: git push
```

#### If You Forget to Move the Bookmark

If you committed but forgot to move `main`, find your commit and move the bookmark:

```bash
# Find your commit
jj log

# Move main to it (use the change ID like 'opzoplvm' or revision like '6e2a5dff')
jj bookmark set main -r <change-id>

# If git HEAD is detached, also run:
git checkout main
```

#### Quick Reference

| Task | Command |
|------|---------|
| See current state | `jj log --limit 5` |
| Commit changes | `jj commit -m "message"` |
| Move main to last commit | `jj bookmark set main -r @-` |
| Push to GitHub | `jj git push` |
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
cargo build --release && cargo test

# Create a review
crit reviews create --title "feat(scope): description of change"
# Note the review ID (e.g., cr-xxxx)
```

**Spawn specialist reviewers** using the code-review skill (`~/.claude/skills/code-review/SKILL.md`):

- **Security reviewer** (always): Looks for injection, auth issues, resource exhaustion, etc.
- **Architecture reviewer** (for structural changes): Evaluates design, abstractions, maintainability

The skill has ready-to-use prompts for spawning these subagents.

### 3. Address Review Feedback

Monitor botbus for reviewer completion:

```bash
botbus history general
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
botbus --version

# Announce on botbus
botbus --agent <your-agent> send botbus "Released vX.Y.Z - [summary of changes]"
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

<!-- crit-agent-instructions -->

## Crit: Agent-Centric Code Review

This project uses [crit](https://github.com/anomalyco/botcrit) for distributed code reviews optimized for AI agents.

### Quick Start

```bash
# Initialize crit in the repository (once)
crit init

# Create a review for current change
crit reviews create --title "Add feature X"

# List open reviews
crit reviews list

# Check reviews needing your attention
crit reviews list --needs-review --author $BOTBUS_AGENT

# Show review details
crit reviews show <review_id>
```

### Adding Comments (Recommended)

The simplest way to comment on code - auto-creates threads:

```bash
# Add a comment on a specific line (creates thread automatically)
crit comment <review_id> --file src/main.rs --line 42 "Consider using Option here"

# Add another comment on same line (reuses existing thread)
crit comment <review_id> --file src/main.rs --line 42 "Good point, will fix"

# Comment on a line range
crit comment <review_id> --file src/main.rs --line 10-20 "This block needs refactoring"
```

### Managing Threads

```bash
# List threads on a review
crit threads list <review_id>

# Show thread with context
crit threads show <thread_id>

# Resolve a thread
crit threads resolve <thread_id> --reason "Fixed in latest commit"
```

### Voting on Reviews

```bash
# Approve a review (LGTM)
crit lgtm <review_id> -m "Looks good!"

# Block a review (request changes)
crit block <review_id> -r "Need more test coverage"
```

### Viewing Full Reviews

```bash
# Show full review with all threads and comments
crit review <review_id>

# Show with more context lines
crit review <review_id> --context 5

# List threads with first comment preview
crit threads list <review_id> -v
```

### Approving and Merging

```bash
# Approve a review (changes status to approved)
crit reviews approve <review_id>

# Mark as merged (after jj squash/merge)
# Note: Will fail if there are blocking votes
crit reviews merge <review_id>

# Self-approve and merge in one step (solo workflows)
crit reviews merge <review_id> --self-approve
```

### Agent Best Practices

1. **Set your identity** using --agent flag or environment variable:
   ```bash
   crit --agent my-agent-name ...
   # Or: export BOTBUS_AGENT=my-agent-name (not persistent in sandboxed environments)
   ```

2. **Check for pending reviews** at session start:
   ```bash
   crit --agent my-agent reviews list --needs-review
   ```

3. **Check status** to see unresolved threads:
   ```bash
   crit status <review_id> --unresolved-only
   ```

4. **Run doctor** to verify setup:
   ```bash
   crit doctor
   ```

### Output Formats

- Default output is TOON (token-optimized, human-readable)
- Use `--json` flag for machine-parseable JSON output

### Key Concepts

- **Reviews** are anchored to jj Change IDs (survive rebases)
- **Threads** group comments on specific file locations
- **crit comment** is the simple way to leave feedback (auto-creates threads)
- Works across jj workspaces (shared .crit/ in main repo)

<!-- end-crit-agent-instructions -->

<!-- maw-agent-instructions-v1 -->

## Multi-Agent Workflow with MAW

This project uses MAW for coordinating multiple agents via jj workspaces.
Each agent gets an isolated working copy and **their own commit** - you can edit files without blocking other agents.

### Quick Start

```bash
maw ws create <your-name>      # Creates workspace + your own commit
cd .workspaces/<your-name>
# ... edit files ...
jj describe -m "feat: what you did"
maw ws status                  # See all agent work
```

### Quick Reference

| Task | Command |
|------|---------|
| Create workspace | `maw ws create <name>` |
| Check status | `maw ws status` |
| Sync stale workspace | `maw ws sync` |
| Run jj in workspace | `maw ws jj <name> <args>` |
| Merge work | `maw ws merge <a> <b>` |
| Destroy workspace | `maw ws destroy <name> --force` |

**Note:** Your workspace starts with an empty commit. This is intentional - it gives you ownership immediately, preventing conflicts when multiple agents work concurrently.

### Session Start

Always run at the beginning of a session:

```bash
maw ws sync                    # Handle stale workspace (safe if not stale)
maw ws status                  # See all agent work
```

### During Work

```bash
maw ws jj <name> diff                        # See changes
maw ws jj <name> log                         # See commit graph
maw ws jj <name> log -r 'working_copies()'   # See all workspace commits
maw ws jj <name> describe -m "feat: ..."     # Save work to your commit
maw ws jj <name> commit -m "feat: ..."       # Commit and start fresh
```

`maw ws jj` runs jj in the workspace directory. Use this instead of `cd .workspaces/<name> && jj ...` — it works reliably in sandboxed environments where cd doesn't persist.

### Stale Workspace

If you see "working copy is stale":

```bash
maw ws sync
```

### Conflicts

jj records conflicts in commits (non-blocking). If you see conflicts:

```bash
jj status                      # Shows conflicted files
# Edit files to resolve
jj describe -m "resolve: ..."
```

### Pushing to Remote (Coordinator)

After merging workspaces, `maw ws merge` checks for push blockers and warns you.
If it reports undescribed commits, fix them before pushing:

```bash
# Option A: rebase merge onto clean base (skips scaffolding commits)
jj rebase -r @- -d main

# Option B: describe the empty commits
jj describe <change-id> -m "workspace setup"
```

Then move the bookmark and push:

```bash
jj bookmark set main -r @-     # Move main to merge commit
jj git push                    # Push to remote
```

<!-- end-maw-agent-instructions -->
