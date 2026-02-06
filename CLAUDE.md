## Pre-commit Checks

```bash
cargo fmt && cargo clippy -- -D warnings && just test
```

CI enforces these. See AGENTS.md for full project context, CLI reference, and architecture.

---

## Version Control: jj (Jujutsu) with Git

This repo uses jj colocated with git. jj creates commits on a "floating" working copy, not directly on branches. **You must move the `main` bookmark after committing.**

### Commit Workflow

```bash
# 1. Commit
jj commit -m "feat(scope): description

Co-Authored-By: Claude <noreply@anthropic.com>"

# 2. Move main bookmark to your commit
jj bookmark set main -r @-

# 3. Push (output says "Changes to push" but it ALREADY pushed)
jj git push

# 4. Verify
git log origin/main --oneline -1
```

### If You Forget to Move the Bookmark

```bash
jj log                             # Find your commit
jj bookmark set main -r <change-id>
jj git push
```

### Quick Reference

| Task | Command |
|------|---------|
| See current state | `jj log --limit 5` |
| Commit changes | `jj commit -m "message"` |
| Move main to last commit | `jj bookmark set main -r @-` |
| Push to GitHub | `jj git push` |
| Verify push succeeded | `git log origin/main --oneline -1` |
| Sync from GitHub | `jj git fetch` then `jj rebase -d main@origin` |

### Commit Conventions

```
<type>(<scope>): <description>

Co-Authored-By: Claude <noreply@anthropic.com>
```

**Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`
**Scopes**: `cli`, `tui`, `storage`, `core`, `sync`, `telegram`, `test`

### Release Workflow

```bash
# Bump version in Cargo.toml, commit, move bookmark, push
jj bookmark set main -r @-
jj git push

# Tag and push tag
jj tag set vX.Y.Z -r main
git push origin vX.Y.Z

# Install and verify
just install && bus --version

# Announce (use --no-hooks to avoid triggering automation)
bus --agent <your-agent> send --no-hooks botbus "Released vX.Y.Z - [summary]"
```

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
