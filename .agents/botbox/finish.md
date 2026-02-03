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

## Assumptions

- `BOTBOX_PROJECT` env var contains the project channel name.
- The workspace was created with `maw ws create --random` during [start](start.md). `$WS` is the workspace name from that step.
