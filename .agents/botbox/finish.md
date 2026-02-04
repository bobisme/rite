# Finish

**Mandatory teardown** after completing work on a bead. Never skip this, even on failure paths.

All steps below are required — they clean up resources, prevent workspace leaks, and ensure the bead ledger stays synchronized. Run **all finish commands** (br, maw, bus) from the **project root**, not from inside `.workspaces/$WS/`. If your shell is cd'd into the workspace, `cd` back to the project root first — `maw ws merge --destroy` deletes the workspace directory and will break your session if you are inside it.

## Arguments

- `$AGENT` = agent identity (required)
- `<bead-id>` = bead to close out (required)

## Steps

1. Resolve agent identity: use `--agent` argument if provided, otherwise `$AGENT` env var. If neither is set, stop and instruct the user. Run `bus whoami --agent $AGENT` first to confirm; if it returns a name, use it.
2. Verify you posted at least one progress comment (`br comments <bead-id>`). If not, add one now: `br comments add --actor $AGENT --author $AGENT <bead-id> "Progress: <what was done>"`
3. Add a completion comment to the bead: `br comments add --actor $AGENT --author $AGENT <bead-id> "Completed by $AGENT"`
4. Close the bead: `br close --actor $AGENT <bead-id> --reason="Completed" --suggest-next`
5. **Merge and destroy the workspace**: `maw ws merge $WS --destroy` (where `$WS` is the workspace name from the start step)
   - The `--destroy` flag is required — it cleans up the workspace after merging
   - If merge fails due to conflicts, do NOT destroy. Instead add a comment: `br comments add --actor $AGENT --author $AGENT <bead-id> "Merge conflict — workspace preserved for manual resolution"` and announce the conflict in the project channel.
   - If the command succeeds but the workspace still exists (`maw ws list`), report: `bus send --agent $AGENT $BOTBOX_PROJECT "Tool issue: maw ws merge --destroy did not remove workspace $WS" -L tool-issue`
6. Release all claims held by this agent: `bus claims release --agent $AGENT --all`
7. Sync the beads ledger: `br sync --flush-only`
8. **If pushMain is enabled** (check `.botbox.json` for `"pushMain": true`), push to GitHub main:
   - `jj bookmark set main -r @-`
   - `jj git push`
   - If push fails, announce: `bus send --agent $AGENT $BOTBOX_PROJECT "Push failed for <bead-id>, manual intervention needed" -L tool-issue`
9. Announce completion in the project channel: `bus send --agent $AGENT $BOTBOX_PROJECT "Completed <bead-id>: <bead-title>" -L task-done`

## After Finishing a Batch of Beads

When you've completed multiple beads in a session (or a significant single bead), check if a **release** is warranted:

**Chores only** (docs, refactoring, config changes, version bumps):
- Push to main is sufficient, no release needed

**Features or fixes** (user-visible changes):
- Follow the project's release process:
  1. Bump version (Cargo.toml, package.json, etc.) using **semantic versioning**
  2. Update changelog/release notes if the project has one
  3. Push to main
  4. Tag the release (`jj tag set vX.Y.Z -r main && git push origin vX.Y.Z`)
  5. Announce on botbus: `bus send --agent $AGENT $BOTBOX_PROJECT "<project> vX.Y.Z released - <summary>" -L release`

Use **conventional commits** (`feat:`, `fix:`, `docs:`, `chore:`, etc.) for clear history.

A "release" = user-visible changes shipped with a version tag. When in doubt, release — it's better to ship small incremental versions than batch up large changes.

## Merge Conflict Recovery

If `maw ws merge` shows "WARNING: Merged workspace has diverged from main":

### Quick fix for .beads/.crit conflicts only

These directories often conflict because multiple agents update them concurrently. If your feature changes are clean and only `.beads/` or `.crit/` conflict:

```bash
jj restore --from main .beads/ .crit/
jj squash
```

Then retry `maw ws merge $WS --destroy`.

### Full recovery if merge is messy

If jj squash times out or multiple commits are tangled:

```bash
# 1. Find your feature commit (has your actual changes)
jj log -r 'all()' --limit 15

# 2. Abandon the merge mess (empty commits, merge commits)
jj abandon <merge-commit-id> <empty-commit-ids>

# 3. Move to your feature commit
jj edit <feature-commit-id>

# 4. Set main and push
jj bookmark set main -r @
jj git push
```

### When to escalate

If recovery takes more than 2-3 attempts, preserve the workspace and escalate:

```bash
br comments add --actor $AGENT --author $AGENT <bead-id> "Merge conflict unresolved. Workspace $WS preserved for manual resolution."
bus send --agent $AGENT $BOTBOX_PROJECT "Merge conflict in $WS for <bead-id>. Manual help needed." -L tool-issue
```

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
- The workspace was created with `maw ws create --random` during [start](start.md). `$WS` is the workspace name from that step.
