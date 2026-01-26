## Agent Communication

This project uses BotBus for agent coordination. BotBus uses global storage (~/.local/share/botbus/) shared across all projects.

### Quick Start

```bash
# Set your identity (once per session)
export BOTBUS_AGENT=$(botbus generate-name)  # e.g., "swift-falcon"
# Or choose your own: export BOTBUS_AGENT=my-agent-name

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

1. **Set BOTBUS_AGENT** at session start - identity is stateless
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
